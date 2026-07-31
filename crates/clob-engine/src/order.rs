//! What a caller can ask the engine to do.

use clob_book::{BaseLots, Side, Ticks};

/// What to do when an order would match against its own owner's resting order.
///
/// Self-matching is not hypothetical: a market maker quoting both sides will cross
/// itself whenever it moves a quote through the touch. Leaving the policy to the caller
/// avoids baking one desk's risk preference into the venue.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SelfTradeBehavior {
    /// Shrink both sides by the overlap without transferring anything. No fee is
    /// charged, since no trade occurred. The default: it preserves the taker's
    /// remaining intent while removing the crossing liquidity.
    #[default]
    DecrementTake,
    /// Cancel the resting order outright and keep matching with undiminished taker
    /// size. What a desk wants when the resting quote is simply stale.
    CancelProvide,
    /// Reject the whole order. For callers who treat a self-match as a bug in their own
    /// quoting logic and would rather fail loudly.
    Abort,
}

/// What to do when a post-only order would cross the book.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PostOnlyRejection {
    /// Reject the order.
    #[default]
    Reject,
    /// Reprice to the best non-crossing tick and post there. Saves a market maker a
    /// round trip when the book moved between quoting and landing, at the cost of
    /// resting at a price it did not name.
    Slide,
}

/// An order as submitted.
///
/// `match_limit` appears on every crossing variant because compute is the binding
/// constraint on-chain: a taker sweeping an unbounded number of price levels can exceed
/// the transaction budget and revert, wasting the fee. Bounding the walk makes the worst
/// case knowable in advance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OrderPacket {
    /// Cross what it can, then rest the remainder.
    Limit {
        /// Which side the order is on.
        side: Side,
        /// Limit price.
        price_in_ticks: Ticks,
        /// Size to fill or post.
        num_base_lots: BaseLots,
        /// Policy for matching against the sender's own orders.
        self_trade_behavior: SelfTradeBehavior,
        /// Maximum resting orders to consume before stopping.
        match_limit: u32,
    },
    /// Rest only. Never takes liquidity, so it never pays a fee.
    PostOnly {
        /// Which side the order is on.
        side: Side,
        /// Limit price.
        price_in_ticks: Ticks,
        /// Size to post.
        num_base_lots: BaseLots,
        /// What to do if this price would cross.
        rejection: PostOnlyRejection,
    },
    /// Cross what it can, then discard the remainder. Never rests.
    ImmediateOrCancel {
        /// Which side the order is on.
        side: Side,
        /// Limit price, or `None` to sweep at any price.
        price_in_ticks: Option<Ticks>,
        /// Size to fill.
        num_base_lots: BaseLots,
        /// Reject the whole order unless at least this much fills. Set equal to
        /// `num_base_lots` for fill-or-kill.
        min_base_lots_to_fill: BaseLots,
        /// Policy for matching against the sender's own orders.
        self_trade_behavior: SelfTradeBehavior,
        /// Maximum resting orders to consume before stopping.
        match_limit: u32,
    },
}

impl OrderPacket {
    /// A plain good-till-cancelled limit order with default policies.
    pub const fn limit(side: Side, price_in_ticks: Ticks, num_base_lots: BaseLots) -> Self {
        Self::Limit {
            side,
            price_in_ticks,
            num_base_lots,
            self_trade_behavior: SelfTradeBehavior::DecrementTake,
            match_limit: u32::MAX,
        }
    }

    /// A post-only order that rejects rather than slides.
    pub const fn post_only(side: Side, price_in_ticks: Ticks, num_base_lots: BaseLots) -> Self {
        Self::PostOnly {
            side,
            price_in_ticks,
            num_base_lots,
            rejection: PostOnlyRejection::Reject,
        }
    }

    /// A market order: sweep up to `num_base_lots` at any price, keep whatever fills.
    pub const fn market(side: Side, num_base_lots: BaseLots) -> Self {
        Self::ImmediateOrCancel {
            side,
            price_in_ticks: None,
            num_base_lots,
            min_base_lots_to_fill: BaseLots::ZERO,
            self_trade_behavior: SelfTradeBehavior::DecrementTake,
            match_limit: u32::MAX,
        }
    }

    /// An all-or-nothing immediate order.
    pub const fn fill_or_kill(side: Side, price_in_ticks: Ticks, num_base_lots: BaseLots) -> Self {
        Self::ImmediateOrCancel {
            side,
            price_in_ticks: Some(price_in_ticks),
            num_base_lots,
            min_base_lots_to_fill: num_base_lots,
            self_trade_behavior: SelfTradeBehavior::DecrementTake,
            match_limit: u32::MAX,
        }
    }

    /// Which side of the book this order is on.
    pub const fn side(&self) -> Side {
        match self {
            Self::Limit { side, .. }
            | Self::PostOnly { side, .. }
            | Self::ImmediateOrCancel { side, .. } => *side,
        }
    }

    /// Total size requested.
    pub const fn num_base_lots(&self) -> BaseLots {
        match self {
            Self::Limit { num_base_lots, .. }
            | Self::PostOnly { num_base_lots, .. }
            | Self::ImmediateOrCancel { num_base_lots, .. } => *num_base_lots,
        }
    }

    /// The limit price, or `None` for an unpriced market order.
    pub const fn price_in_ticks(&self) -> Option<Ticks> {
        match self {
            Self::Limit { price_in_ticks, .. } | Self::PostOnly { price_in_ticks, .. } => {
                Some(*price_in_ticks)
            }
            Self::ImmediateOrCancel { price_in_ticks, .. } => *price_in_ticks,
        }
    }

    /// Whether any part of this order may rest on the book.
    pub const fn can_post(&self) -> bool {
        matches!(self, Self::Limit { .. } | Self::PostOnly { .. })
    }

    /// Whether this order may take liquidity.
    pub const fn can_take(&self) -> bool {
        matches!(self, Self::Limit { .. } | Self::ImmediateOrCancel { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_only_never_takes_and_ioc_never_posts() {
        let post = OrderPacket::post_only(Side::Bid, Ticks(100), BaseLots(1));
        assert!(post.can_post() && !post.can_take());

        let ioc = OrderPacket::market(Side::Bid, BaseLots(1));
        assert!(ioc.can_take() && !ioc.can_post());

        let limit = OrderPacket::limit(Side::Bid, Ticks(100), BaseLots(1));
        assert!(limit.can_post() && limit.can_take());
    }

    #[test]
    fn a_market_order_has_no_limit_price() {
        assert_eq!(OrderPacket::market(Side::Ask, BaseLots(1)).price_in_ticks(), None);
        assert_eq!(
            OrderPacket::limit(Side::Ask, Ticks(7), BaseLots(1)).price_in_ticks(),
            Some(Ticks(7))
        );
    }

    #[test]
    fn fill_or_kill_requires_the_whole_size() {
        let packet = OrderPacket::fill_or_kill(Side::Bid, Ticks(100), BaseLots(9));
        let OrderPacket::ImmediateOrCancel {
            min_base_lots_to_fill,
            ..
        } = packet
        else {
            panic!("expected an IOC packet");
        };
        assert_eq!(min_base_lots_to_fill, BaseLots(9));
    }
}
