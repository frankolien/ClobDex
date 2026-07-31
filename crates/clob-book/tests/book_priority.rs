//! Price-time priority and size conservation under arbitrary book activity.
//!
//! Priority is the property a venue is judged on: a maker who quoted first at a price
//! must be filled first, or the venue is broken in a way no amount of throughput makes
//! up for. Here it is checked against an independently sorted model rather than against
//! the tree's own ordering, so a bug in the [`FIFOOrderId`] encoding cannot hide behind
//! a matching bug in the tree.
//!
//! [`FIFOOrderId`]: clob_book::FIFOOrderId

use clob_book::{BaseLots, FIFOOrderId, OrderBook, RestingOrder, Side, Ticks};
use proptest::prelude::*;

const BIDS: usize = 48;
const ASKS: usize = 48;

type Book = OrderBook<BIDS, ASKS>;

#[derive(Debug, Clone)]
enum Op {
    /// Rest an order. The index selects which live order a later cancel/reduce targets.
    Place { side: Side, price: u64, size: u64 },
    Cancel { index: usize },
    Reduce { index: usize, size: u64 },
}

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Bid), Just(Side::Ask)]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        10 => (side(), 95u64..106, 1u64..50)
            .prop_map(|(side, price, size)| Op::Place { side, price, size }),
        3 => any::<prop::sample::Index>().prop_map(|i| Op::Cancel { index: i.index(usize::MAX) }),
        3 => (any::<prop::sample::Index>(), 1u64..50)
            .prop_map(|(i, size)| Op::Reduce { index: i.index(usize::MAX), size }),
    ]
}

/// The model: every live order, in placement order.
type Model = Vec<(FIFOOrderId, BaseLots)>;

/// Sorts a side's live orders into the priority a correct book must produce: best price
/// first, then oldest first. Computed from raw prices and sequence numbers, entirely
/// independently of the bit-inversion encoding under test.
fn expected_priority(model: &Model, side: Side) -> Vec<FIFOOrderId> {
    let mut live: Vec<FIFOOrderId> = model
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| id.side() == side)
        .collect();

    live.sort_by(|a, b| {
        let price = match side {
            Side::Bid => b.price_in_ticks.cmp(&a.price_in_ticks),
            Side::Ask => a.price_in_ticks.cmp(&b.price_in_ticks),
        };
        price.then(a.sequence_number().cmp(&b.sequence_number()))
    });
    live
}

fn total_size(model: &Model, side: Side) -> BaseLots {
    model
        .iter()
        .filter(|(id, _)| id.side() == side)
        .fold(BaseLots::ZERO, |sum, (_, size)| sum.saturating_add(*size))
}

fn apply(book: &mut Book, model: &mut Model, op: &Op) {
    match *op {
        Op::Place { side, price, size } => {
            let order = RestingOrder::new(model.len() as u64, BaseLots(size));
            if let Some(id) = book.place(side, Ticks(price), order) {
                model.push((id, BaseLots(size)));
            }
        }
        Op::Cancel { index } => {
            if model.is_empty() {
                return;
            }
            let (id, _) = model.remove(index % model.len());
            book.cancel(&id);
        }
        Op::Reduce { index, size } => {
            if model.is_empty() {
                return;
            }
            let slot = index % model.len();
            let (id, resting) = model[slot];
            let removed = resting.min(BaseLots(size));
            let remaining = resting - removed;

            book.reduce(&id, BaseLots(size));

            if remaining.is_zero() {
                model.remove(slot);
            } else {
                model[slot].1 = remaining;
            }
        }
    }
}

fn assert_agrees(book: &Book, model: &Model) -> Result<(), TestCaseError> {
    prop_assert_eq!(book.check(), Ok(()));

    for side in [Side::Bid, Side::Ask] {
        let actual: Vec<FIFOOrderId> = book.iter_side(side).map(|e| e.key).collect();
        prop_assert_eq!(&actual, &expected_priority(model, side), "priority diverged");

        prop_assert_eq!(book.len(side), actual.len());
        prop_assert_eq!(book.best(side).map(|e| e.key), actual.first().copied());
        // Conservation: nothing appears or vanishes without an operation causing it.
        prop_assert_eq!(book.total_depth(side), total_size(model, side));
    }

    for (id, size) in model {
        prop_assert!(book.contains(id));
        prop_assert_eq!(book.get(id).map(|o| o.num_base_lots), Some(*size));
    }

    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The headline property: priority and size are exactly what an independent model
    /// says they should be, after every operation.
    #[test]
    fn priority_and_size_track_the_model(ops in prop::collection::vec(op(), 0..300)) {
        let mut book = Book::new_boxed();
        let mut model = Model::new();

        for op in &ops {
            apply(&mut book, &mut model, op);
            assert_agrees(&book, &model)?;
        }
    }

    /// Orders placed at one price are filled strictly in arrival order, no matter what
    /// else is happening around them. This is the property makers care about most.
    #[test]
    fn equal_prices_are_strictly_first_in_first_out(
        count in 1usize..20,
        // Deliberately excludes 100 so noise never lands in the level under test.
        noise in prop::collection::vec(prop_oneof![96u64..100, 101u64..105], 0..20),
    ) {
        let mut book = Book::new_boxed();

        // Interleave the orders under test with noise at other prices.
        let mut placed = Vec::new();
        for i in 0..count {
            if let Some(price) = noise.get(i) {
                book.place(Side::Bid, Ticks(*price), RestingOrder::new(999, BaseLots(1)));
            }
            let id = book
                .place(Side::Bid, Ticks(100), RestingOrder::new(i as u64, BaseLots(1)))
                .expect("capacity");
            placed.push(id);
        }

        let at_price: Vec<FIFOOrderId> = book
            .iter_side(Side::Bid)
            .map(|e| e.key)
            .filter(|id| id.price_in_ticks == Ticks(100))
            .collect();

        prop_assert_eq!(at_price, placed);
    }

    /// Reducing an order must never cost it queue position — a maker topping up or
    /// trimming size should not be sent to the back of the queue.
    #[test]
    fn reducing_preserves_queue_position(
        depth in 2usize..12,
        target in 0usize..12,
        reduce_by in 1u64..5,
    ) {
        let mut book = Book::new_boxed();
        let ids: Vec<FIFOOrderId> = (0..depth)
            .map(|i| {
                book.place(Side::Ask, Ticks(100), RestingOrder::new(i as u64, BaseLots(10)))
                    .expect("capacity")
            })
            .collect();

        let target = target % depth;
        book.reduce(&ids[target], BaseLots(reduce_by)).expect("order is live");

        let actual: Vec<FIFOOrderId> = book.iter_side(Side::Ask).map(|e| e.key).collect();
        prop_assert_eq!(actual, ids);
    }

    /// Sequence numbers are unique across the whole market and strictly increasing, so
    /// an ID can never be ambiguous between the two sides.
    #[test]
    fn sequence_numbers_are_globally_unique_and_monotonic(
        sides in prop::collection::vec(side(), 1..60),
    ) {
        let mut book = Book::new_boxed();
        let mut previous = None;

        for (i, side) in sides.iter().enumerate() {
            let Some(id) = book.place(*side, Ticks(100 + i as u64 % 8), RestingOrder::new(0, BaseLots(1)))
            else {
                continue; // Side at capacity; nothing to assert.
            };

            prop_assert_eq!(id.side(), *side, "side tag lost in the encoding");
            if let Some(previous) = previous {
                prop_assert!(id.sequence_number() > previous, "sequence went backwards");
            }
            previous = Some(id.sequence_number());
        }
    }
}
