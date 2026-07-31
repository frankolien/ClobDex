//! The two-sided book.
//!
//! [`OrderBook`] owns both trees and the sequence counter. It implements no matching
//! policy — no order types, no self-trade rules, no fees — only what an engine needs to
//! read and mutate a book: placement, cancellation, reduction, top-of-book, and depth.

mod depth;

use bytemuck::{Pod, Zeroable};

use crate::order::{FIFOOrderId, RestingOrder, Side};
use crate::quantities::{BaseLots, Ticks};
use crate::tree::{Entry, Invariant, NIL, RedBlackTree};

/// An entry in either side of the book.
pub type BookEntry = Entry<FIFOOrderId, RestingOrder>;

/// The outcome of reducing a resting order.
///
/// Carries enough to emit an event and credit funds without a second lookup, which is
/// what a cancel or reduce instruction needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Reduction {
    /// Size actually taken off the book. Less than requested if the order was smaller.
    pub base_lots_removed: BaseLots,
    /// Size still resting. Zero exactly when `removed` is true.
    pub base_lots_remaining: BaseLots,
    /// Whether the order left the book entirely.
    pub removed: bool,
}

/// A two-sided limit order book with fixed per-side capacity.
///
/// `BIDS` and `ASKS` are capacities in orders. Sizing them separately is deliberate:
/// many markets are structurally lopsided (an LST/SOL pair quotes far more depth on one
/// side), and paying symmetric rent for asymmetric books is pure waste.
///
/// Both sides draw sequence numbers from one shared counter, so a raw sequence number
/// identifies an order uniquely across the whole market, not just within a side.
#[repr(C)]
pub struct OrderBook<const BIDS: usize, const ASKS: usize> {
    /// Next raw sequence number to issue. Monotonic for the life of the market.
    next_sequence_number: u64,
    bids: RedBlackTree<FIFOOrderId, RestingOrder, BIDS>,
    asks: RedBlackTree<FIFOOrderId, RestingOrder, ASKS>,
}

impl<const BIDS: usize, const ASKS: usize> Clone for OrderBook<BIDS, ASKS> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<const BIDS: usize, const ASKS: usize> Copy for OrderBook<BIDS, ASKS> {}

// SAFETY: `repr(C)` over a `u64` and two `Pod` trees, each 8-aligned with a size that is
// a multiple of 8. No padding anywhere; `ASSERT_NO_PADDING` verifies it.
unsafe impl<const BIDS: usize, const ASKS: usize> Zeroable for OrderBook<BIDS, ASKS> {}
unsafe impl<const BIDS: usize, const ASKS: usize> Pod for OrderBook<BIDS, ASKS> {}

impl<const BIDS: usize, const ASKS: usize> Default for OrderBook<BIDS, ASKS> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------
// Construction and access
// ---------------------------------------------------------------------------------

impl<const BIDS: usize, const ASKS: usize> OrderBook<BIDS, ASKS> {
    const ASSERT_NO_PADDING: () = assert!(
        core::mem::size_of::<Self>()
            == 8
                + RedBlackTree::<FIFOOrderId, RestingOrder, BIDS>::SIZE_IN_BYTES
                + RedBlackTree::<FIFOOrderId, RestingOrder, ASKS>::SIZE_IN_BYTES,
        "OrderBook has padding between its counter and its trees"
    );

    /// Account space this book needs, in bytes.
    pub const SIZE_IN_BYTES: usize = core::mem::size_of::<Self>();

    /// An empty book.
    ///
    /// Beware the large stack temporary; prefer [`OrderBook::new_boxed`] off-chain and a
    /// cast from account bytes on-chain.
    #[inline]
    pub fn new() -> Self {
        let () = Self::ASSERT_NO_PADDING;
        Zeroable::zeroed()
    }

    /// An empty book on the heap.
    #[cfg(feature = "std")]
    #[inline]
    pub fn new_boxed() -> std::boxed::Box<Self> {
        let () = Self::ASSERT_NO_PADDING;
        bytemuck::zeroed_box()
    }

    /// The bid tree, ordered ascending by price — so the best bid is its maximum.
    #[inline(always)]
    pub const fn bids(&self) -> &RedBlackTree<FIFOOrderId, RestingOrder, BIDS> {
        &self.bids
    }

    /// The ask tree, ordered ascending by price — so the best ask is its minimum.
    #[inline(always)]
    pub const fn asks(&self) -> &RedBlackTree<FIFOOrderId, RestingOrder, ASKS> {
        &self.asks
    }

    /// Mutable access to the bid tree, for engine-layer operations this crate does not
    /// implement.
    #[inline(always)]
    pub const fn bids_mut(&mut self) -> &mut RedBlackTree<FIFOOrderId, RestingOrder, BIDS> {
        &mut self.bids
    }

    /// Mutable access to the ask tree.
    #[inline(always)]
    pub const fn asks_mut(&mut self) -> &mut RedBlackTree<FIFOOrderId, RestingOrder, ASKS> {
        &mut self.asks
    }

    /// Resting orders on `side`.
    #[inline]
    pub const fn len(&self, side: Side) -> usize {
        match side {
            Side::Bid => self.bids.len(),
            Side::Ask => self.asks.len(),
        }
    }

    /// Whether `side` holds no orders.
    #[inline]
    pub const fn is_empty(&self, side: Side) -> bool {
        self.len(side) == 0
    }

    /// Whether `side` is at capacity, so the next placement there will fail.
    #[inline]
    pub const fn is_full(&self, side: Side) -> bool {
        match side {
            Side::Bid => self.bids.is_full(),
            Side::Ask => self.asks.is_full(),
        }
    }

    /// The next sequence number that will be issued.
    #[inline(always)]
    pub const fn next_sequence_number(&self) -> u64 {
        self.next_sequence_number
    }

    /// Empties both sides. The sequence counter is *not* reset, so order IDs stay
    /// unique across the life of the market and a stale client cannot accidentally
    /// cancel a new order with an old ID.
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
    }
}

// ---------------------------------------------------------------------------------
// Mutation
// ---------------------------------------------------------------------------------

impl<const BIDS: usize, const ASKS: usize> OrderBook<BIDS, ASKS> {
    /// Rests an order on `side` at `price`, assigning it the next sequence number.
    ///
    /// Returns the new ID, or `None` if that side is at capacity. Pure placement — it
    /// does not check whether the order crosses. That is the engine's call.
    pub fn place(&mut self, side: Side, price: Ticks, order: RestingOrder) -> Option<FIFOOrderId> {
        let id = FIFOOrderId::new(side, price, self.next_sequence_number);

        let handle = match side {
            Side::Bid => self.bids.insert(id, order),
            Side::Ask => self.asks.insert(id, order),
        };

        // Only burn a sequence number if the order actually rested, so a full book does
        // not advance the counter.
        handle.map(|_| {
            self.next_sequence_number += 1;
            id
        })
    }

    /// Removes an order, returning its resting state if it was there.
    ///
    /// The side is read from the ID's encoded side tag, so a caller never has to pass it
    /// separately or get it wrong.
    pub fn cancel(&mut self, id: &FIFOOrderId) -> Option<RestingOrder> {
        match id.side() {
            Side::Bid => self.bids.remove(id),
            Side::Ask => self.asks.remove(id),
        }
    }

    /// Looks up a resting order.
    #[inline]
    pub fn get(&self, id: &FIFOOrderId) -> Option<&RestingOrder> {
        match id.side() {
            Side::Bid => self.bids.get(id),
            Side::Ask => self.asks.get(id),
        }
    }

    /// Mutable access to a resting order.
    ///
    /// Safe because nothing in [`RestingOrder`] participates in the ordering, so no
    /// caller can invalidate the book through this. Partial fills use it to shrink an
    /// order in place without disturbing its queue position.
    #[inline]
    pub fn get_mut(&mut self, id: &FIFOOrderId) -> Option<&mut RestingOrder> {
        match id.side() {
            Side::Bid => self.bids.get_mut(id),
            Side::Ask => self.asks.get_mut(id),
        }
    }

    /// Shrinks an order by up to `base_lots`, removing it if that empties it.
    ///
    /// Reducing by at least the resting size is exactly a cancel. Returns `None` if the
    /// order is not on the book.
    pub fn reduce(&mut self, id: &FIFOOrderId, base_lots: BaseLots) -> Option<Reduction> {
        let resting = self.get(id)?.num_base_lots;
        let base_lots_removed = resting.min(base_lots);
        let base_lots_remaining = resting - base_lots_removed;

        if base_lots_remaining.is_zero() {
            self.cancel(id);
        } else {
            // The lookup above found it and nothing has mutated since, so this is
            // always `Some`. Propagating rather than unwrapping keeps the hot path
            // panic-free, which matters more on-chain than flagging the impossible.
            self.get_mut(id)?.num_base_lots = base_lots_remaining;
        }

        Some(Reduction {
            base_lots_removed,
            base_lots_remaining,
            removed: base_lots_remaining.is_zero(),
        })
    }
}

// ---------------------------------------------------------------------------------
// Top of book
// ---------------------------------------------------------------------------------

impl<const BIDS: usize, const ASKS: usize> OrderBook<BIDS, ASKS> {
    /// The highest-priority bid: highest price, and oldest within that price.
    ///
    /// This is the bid tree's *maximum*, courtesy of the sequence-number encoding in
    /// [`FIFOOrderId`].
    #[inline]
    pub fn best_bid(&self) -> Option<BookEntry> {
        self.bids.last()
    }

    /// The highest-priority ask: lowest price, and oldest within that price.
    #[inline]
    pub fn best_ask(&self) -> Option<BookEntry> {
        self.asks.first()
    }

    /// The best order on `side`.
    #[inline]
    pub fn best(&self, side: Side) -> Option<BookEntry> {
        match side {
            Side::Bid => self.best_bid(),
            Side::Ask => self.best_ask(),
        }
    }

    /// Best ask minus best bid, in ticks. `None` if either side is empty.
    ///
    /// Returns `Some(0)` for a locked book and `None` — not a negative number — for a
    /// crossed one; use [`OrderBook::is_crossed`] to distinguish. A crossed book is not
    /// an error at this layer: it is the transient state an engine sees mid-match.
    #[inline]
    pub fn spread_in_ticks(&self) -> Option<u64> {
        let bid = self.best_bid()?.key.price_in_ticks;
        let ask = self.best_ask()?.key.price_in_ticks;
        ask.as_u64().checked_sub(bid.as_u64())
    }

    /// Whether the best bid is at or above the best ask, i.e. there is liquidity to
    /// match. Always false if either side is empty.
    #[inline]
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => bid.key.price_in_ticks >= ask.key.price_in_ticks,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------

impl<const BIDS: usize, const ASKS: usize> OrderBook<BIDS, ASKS> {
    /// Checks both trees' structural invariants, plus the book-level rule that every
    /// order is filed on the side its ID claims.
    ///
    /// See [`RedBlackTree::check`] for intended use.
    ///
    /// # Errors
    ///
    /// The first [`Invariant`] found violated.
    pub fn check(&self) -> Result<(), Invariant> {
        self.bids.check()?;
        self.asks.check()?;

        let misfiled = self.bids.iter().any(|e| e.key.side() != Side::Bid)
            || self.asks.iter().any(|e| e.key.side() != Side::Ask);
        if misfiled {
            return Err(Invariant::BstOrderViolation);
        }

        Ok(())
    }

    /// Whether an ID refers to a live order.
    #[inline]
    pub fn contains(&self, id: &FIFOOrderId) -> bool {
        match id.side() {
            Side::Bid => self.bids.find(id) != NIL,
            Side::Ask => self.asks.find(id) != NIL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Book = OrderBook<32, 32>;

    #[test]
    fn zeroed_bytes_are_a_valid_empty_book() {
        let bytes = std::vec![0u8; Book::SIZE_IN_BYTES];
        let book: &Book = bytemuck::from_bytes(&bytes);

        assert!(book.is_empty(Side::Bid));
        assert!(book.is_empty(Side::Ask));
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.check(), Ok(()));
    }

    #[test]
    fn best_bid_is_highest_price_then_oldest() {
        let mut book = Book::new_boxed();
        book.place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(1)));
        let first_at_top = book
            .place(Side::Bid, Ticks(101), RestingOrder::new(1, BaseLots(2)))
            .unwrap();
        book.place(Side::Bid, Ticks(101), RestingOrder::new(2, BaseLots(3)));
        book.place(Side::Bid, Ticks(99), RestingOrder::new(3, BaseLots(4)));

        let best = book.best_bid().unwrap();
        assert_eq!(best.key, first_at_top);
        assert_eq!(best.key.price_in_ticks, Ticks(101));
        assert_eq!(best.value.trader_index, 1);
    }

    #[test]
    fn best_ask_is_lowest_price_then_oldest() {
        let mut book = Book::new_boxed();
        book.place(Side::Ask, Ticks(105), RestingOrder::new(0, BaseLots(1)));
        let first_at_top = book
            .place(Side::Ask, Ticks(103), RestingOrder::new(1, BaseLots(2)))
            .unwrap();
        book.place(Side::Ask, Ticks(103), RestingOrder::new(2, BaseLots(3)));

        let best = book.best_ask().unwrap();
        assert_eq!(best.key, first_at_top);
        assert_eq!(best.value.trader_index, 1);
    }

    #[test]
    fn sides_share_one_sequence_counter() {
        let mut book = Book::new_boxed();
        let bid = book
            .place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(1)))
            .unwrap();
        let ask = book
            .place(Side::Ask, Ticks(101), RestingOrder::new(0, BaseLots(1)))
            .unwrap();

        assert_eq!(bid.sequence_number(), 0);
        assert_eq!(ask.sequence_number(), 1);
        assert_eq!(book.next_sequence_number(), 2);
    }

    #[test]
    fn a_full_side_neither_rests_nor_burns_a_sequence_number() {
        let mut book = OrderBook::<2, 2>::new_boxed();
        for trader in 0..2 {
            assert!(
                book.place(Side::Bid, Ticks(100 + trader), RestingOrder::new(trader, BaseLots(1)))
                    .is_some()
            );
        }

        let sequence_before = book.next_sequence_number();
        assert_eq!(
            book.place(Side::Bid, Ticks(200), RestingOrder::new(9, BaseLots(1))),
            None
        );
        assert_eq!(book.next_sequence_number(), sequence_before);

        // The other side is unaffected by a full book.
        assert!(book.place(Side::Ask, Ticks(300), RestingOrder::new(9, BaseLots(1))).is_some());
    }

    #[test]
    fn cancel_routes_by_the_encoded_side() {
        let mut book = Book::new_boxed();
        let bid = book
            .place(Side::Bid, Ticks(100), RestingOrder::new(7, BaseLots(5)))
            .unwrap();
        let ask = book
            .place(Side::Ask, Ticks(101), RestingOrder::new(8, BaseLots(6)))
            .unwrap();

        assert_eq!(book.cancel(&bid).unwrap().trader_index, 7);
        assert_eq!(book.cancel(&ask).unwrap().trader_index, 8);
        assert_eq!(book.cancel(&bid), None);
        assert_eq!(book.check(), Ok(()));
    }

    #[test]
    fn partial_reduction_keeps_the_order_and_its_priority() {
        let mut book = Book::new_boxed();
        let first = book
            .place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(10)))
            .unwrap();
        book.place(Side::Bid, Ticks(100), RestingOrder::new(1, BaseLots(10)));

        let reduction = book.reduce(&first, BaseLots(4)).unwrap();

        assert_eq!(
            reduction,
            Reduction {
                base_lots_removed: BaseLots(4),
                base_lots_remaining: BaseLots(6),
                removed: false,
            }
        );
        // Still at the front of the queue: reducing must not cost time priority.
        assert_eq!(book.best_bid().unwrap().key, first);
        assert_eq!(book.get(&first).unwrap().num_base_lots, BaseLots(6));
    }

    #[test]
    fn over_reduction_clamps_and_removes() {
        let mut book = Book::new_boxed();
        let id = book
            .place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(3)))
            .unwrap();

        let reduction = book.reduce(&id, BaseLots(999)).unwrap();

        assert_eq!(
            reduction,
            Reduction {
                base_lots_removed: BaseLots(3),
                base_lots_remaining: BaseLots::ZERO,
                removed: true,
            }
        );
        assert!(!book.contains(&id));
        assert_eq!(book.reduce(&id, BaseLots(1)), None);
    }

    #[test]
    fn spread_and_cross_detection() {
        let mut book = Book::new_boxed();
        assert_eq!(book.spread_in_ticks(), None);
        assert!(!book.is_crossed());

        book.place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(1)));
        assert_eq!(book.spread_in_ticks(), None, "one-sided book has no spread");

        book.place(Side::Ask, Ticks(103), RestingOrder::new(1, BaseLots(1)));
        assert_eq!(book.spread_in_ticks(), Some(3));
        assert!(!book.is_crossed());

        book.place(Side::Bid, Ticks(105), RestingOrder::new(2, BaseLots(1)));
        assert!(book.is_crossed());
        assert_eq!(book.spread_in_ticks(), None, "crossed book has no non-negative spread");
    }

    #[test]
    fn clear_empties_the_book_but_not_the_counter() {
        let mut book = Book::new_boxed();
        book.place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(1)));
        book.place(Side::Ask, Ticks(101), RestingOrder::new(1, BaseLots(1)));

        book.clear();

        assert!(book.is_empty(Side::Bid) && book.is_empty(Side::Ask));
        // Sequence numbers must never be reused, or a stale client could cancel an
        // order it does not own.
        assert_eq!(book.next_sequence_number(), 2);
        let reused = book
            .place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(1)))
            .unwrap();
        assert_eq!(reused.sequence_number(), 2);
    }
}
