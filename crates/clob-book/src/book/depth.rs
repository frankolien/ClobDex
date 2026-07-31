//! Depth and liquidity queries.
//!
//! These stop at the first price that fails the predicate, so cost scales with the
//! depth inspected rather than the size of the book.

use super::{BookEntry, OrderBook};
use crate::order::Side;
use crate::quantities::{BaseLots, Ticks};

impl<const BIDS: usize, const ASKS: usize> OrderBook<BIDS, ASKS> {
    /// Orders on `side` in the sequence a taker consumes them: bids descending by
    /// price, asks ascending.
    pub fn iter_side(&self, side: Side) -> impl Iterator<Item = BookEntry> + '_ {
        // The two sides have different iterator types. Chaining an empty one of each
        // unifies them without boxing.
        let bids = side.is_bid().then(|| self.bids().iter_rev()).into_iter().flatten();
        let asks = (!side.is_bid()).then(|| self.asks().iter()).into_iter().flatten();
        bids.chain(asks)
    }

    /// Whether a taker hitting `side` with limit `limit` would accept `price`.
    #[inline(always)]
    const fn is_marketable(side: Side, price: Ticks, limit: Ticks) -> bool {
        match side {
            Side::Bid => price.as_u64() >= limit.as_u64(),
            Side::Ask => price.as_u64() <= limit.as_u64(),
        }
    }

    /// Total size on `side` at prices at or better than `limit` — what a taker could
    /// sweep, ignoring fees and self-trade rules.
    pub fn depth_at_or_better(&self, side: Side, limit: Ticks) -> BaseLots {
        self.iter_side(side)
            .take_while(|entry| Self::is_marketable(side, entry.key.price_in_ticks, limit))
            .fold(BaseLots::ZERO, |total, entry| {
                total.saturating_add(entry.value.num_base_lots)
            })
    }

    /// Total size on `side` at any price.
    pub fn total_depth(&self, side: Side) -> BaseLots {
        self.iter_side(side).fold(BaseLots::ZERO, |total, entry| {
            total.saturating_add(entry.value.num_base_lots)
        })
    }

    /// The worst price a taker reaches sweeping `size` from `side`, or `None` if the
    /// book cannot fill that much. Answers price impact without simulating a match.
    pub fn sweep_price(&self, side: Side, size: BaseLots) -> Option<Ticks> {
        let mut remaining = size;
        for entry in self.iter_side(side) {
            remaining = remaining.saturating_sub(entry.value.num_base_lots);
            if remaining.is_zero() {
                return Some(entry.key.price_in_ticks);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::RestingOrder;

    type Book = OrderBook<32, 32>;

    /// Bids at 100/99/98 and asks at 102/103/104, ten lots each.
    fn ladder() -> std::boxed::Box<Book> {
        let mut book = Book::new_boxed();
        for (index, price) in [100u64, 99, 98].into_iter().enumerate() {
            book.place(Side::Bid, Ticks(price), RestingOrder::new(index as u64, BaseLots(10)));
        }
        for (index, price) in [102u64, 103, 104].into_iter().enumerate() {
            book.place(Side::Ask, Ticks(price), RestingOrder::new(index as u64, BaseLots(10)));
        }
        book
    }

    #[test]
    fn iteration_starts_at_the_touch_on_both_sides() {
        let book = ladder();
        let bids: std::vec::Vec<u64> = book
            .iter_side(Side::Bid)
            .map(|e| e.key.price_in_ticks.as_u64())
            .collect();
        let asks: std::vec::Vec<u64> = book
            .iter_side(Side::Ask)
            .map(|e| e.key.price_in_ticks.as_u64())
            .collect();

        assert_eq!(bids, std::vec![100, 99, 98]);
        assert_eq!(asks, std::vec![102, 103, 104]);
    }

    #[test]
    fn depth_accumulates_only_marketable_levels() {
        let book = ladder();

        // A seller with a limit of 99 reaches the 100 and 99 bids, not the 98.
        assert_eq!(book.depth_at_or_better(Side::Bid, Ticks(99)), BaseLots(20));
        assert_eq!(book.depth_at_or_better(Side::Ask, Ticks(103)), BaseLots(20));
    }

    #[test]
    fn an_unmarketable_limit_finds_no_depth() {
        let book = ladder();
        assert_eq!(book.depth_at_or_better(Side::Bid, Ticks(101)), BaseLots::ZERO);
        assert_eq!(book.depth_at_or_better(Side::Ask, Ticks(101)), BaseLots::ZERO);
    }

    #[test]
    fn total_depth_covers_every_level() {
        let book = ladder();
        assert_eq!(book.total_depth(Side::Bid), BaseLots(30));
        assert_eq!(book.total_depth(Side::Ask), BaseLots(30));
    }

    #[test]
    fn sweep_price_reports_the_worst_level_touched() {
        let book = ladder();

        assert_eq!(book.sweep_price(Side::Ask, BaseLots(10)), Some(Ticks(102)));
        // One lot past the top level reaches the next.
        assert_eq!(book.sweep_price(Side::Ask, BaseLots(11)), Some(Ticks(103)));
        assert_eq!(book.sweep_price(Side::Bid, BaseLots(30)), Some(Ticks(98)));
        assert_eq!(book.sweep_price(Side::Bid, BaseLots(31)), None);
    }

    #[test]
    fn an_empty_book_has_no_depth_and_no_sweep_price() {
        let book = Book::new_boxed();
        assert_eq!(book.total_depth(Side::Bid), BaseLots::ZERO);
        assert_eq!(book.sweep_price(Side::Ask, BaseLots(1)), None);
    }
}
