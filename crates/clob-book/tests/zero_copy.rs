//! The layout guarantees that let a market account *be* a book.
//!
//! On-chain there is no deserialization step: an instruction casts the account's byte
//! slice to an [`OrderBook`] and operates on it in place. That only works if the layout
//! is exactly what we think it is, so these tests assert the byte-level contract that
//! the rest of the design rests on.

use clob_book::{
    BaseLots, FIFOOrderId, LotConfig, OrderBook, RedBlackTree, RestingOrder, Side, Ticks,
};

type Book = OrderBook<64, 64>;
type Tree = RedBlackTree<FIFOOrderId, RestingOrder, 64>;

#[test]
fn sizes_are_exactly_the_header_plus_the_arena() {
    // 16-byte tree header; slots of 16 link bytes + 16-byte key + 32-byte value.
    const SLOT: usize = 64;
    assert_eq!(Tree::SIZE_IN_BYTES, 16 + 64 * SLOT);
    // Book header is the 8-byte sequence counter, then both trees back to back.
    assert_eq!(Book::SIZE_IN_BYTES, 8 + 2 * Tree::SIZE_IN_BYTES);
}

#[test]
fn a_market_slot_is_one_cache_line() {
    // Not required for correctness, but it is why the arena is laid out this way: a
    // 64-byte slot means one node visit is one cache line, and tree depth translates
    // directly into cache misses.
    assert_eq!(Tree::SIZE_IN_BYTES - 16, 64 * 64);
}

#[test]
fn alignment_is_eight_so_account_data_can_be_cast_directly() {
    assert_eq!(align_of::<Book>(), 8);
    assert_eq!(align_of::<Tree>(), 8);
    assert_eq!(align_of::<FIFOOrderId>(), 8);
    assert_eq!(align_of::<RestingOrder>(), 8);
    assert_eq!(align_of::<LotConfig>(), 8);
}

#[test]
fn a_zeroed_account_is_a_usable_market() {
    // This is exactly what `SystemProgram::CreateAccount` hands back: zeroed bytes.
    let mut bytes = vec![0u8; Book::SIZE_IN_BYTES];
    let book: &mut Book = bytemuck::from_bytes_mut(&mut bytes);

    assert_eq!(book.check(), Ok(()));
    assert!(book.is_empty(Side::Bid) && book.is_empty(Side::Ask));

    // No initialization instruction ran; the book is immediately usable.
    let id = book
        .place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(5)))
        .expect("a zeroed book has full capacity");
    assert_eq!(book.best_bid().unwrap().key, id);
}

#[test]
fn mutations_land_in_the_underlying_bytes() {
    let mut bytes = vec![0u8; Book::SIZE_IN_BYTES];

    let id = {
        let book: &mut Book = bytemuck::from_bytes_mut(&mut bytes);
        book.place(Side::Ask, Ticks(250), RestingOrder::new(42, BaseLots(7)))
            .unwrap()
    };

    // Simulating the next transaction: a fresh cast over the same account data.
    let reloaded: &Book = bytemuck::from_bytes(&bytes);

    assert_eq!(reloaded.check(), Ok(()));
    assert_eq!(reloaded.best_ask().unwrap().key, id);
    assert_eq!(reloaded.get(&id).unwrap().trader_index, 42);
    assert_eq!(reloaded.next_sequence_number(), 1);
}

#[test]
fn state_survives_a_full_serialize_deserialize_cycle() {
    let mut book = Book::new_boxed();
    let mut ids = Vec::new();
    for i in 0..20u64 {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let price = if side == Side::Bid { 100 - i } else { 200 + i };
        ids.push(
            book.place(side, Ticks(price), RestingOrder::new(i, BaseLots(i + 1)))
                .unwrap(),
        );
    }
    for id in ids.iter().step_by(3) {
        book.cancel(id);
    }

    let bytes = bytemuck::bytes_of(book.as_ref()).to_vec();
    let restored: &Book = bytemuck::from_bytes(&bytes);

    assert_eq!(restored.check(), Ok(()));
    assert_eq!(restored.best_bid().map(|e| e.key), book.best_bid().map(|e| e.key));
    assert_eq!(restored.total_depth(Side::Bid), book.total_depth(Side::Bid));
    assert_eq!(restored.total_depth(Side::Ask), book.total_depth(Side::Ask));
    assert_eq!(
        restored.iter_side(Side::Bid).map(|e| e.key).collect::<Vec<_>>(),
        book.iter_side(Side::Bid).map(|e| e.key).collect::<Vec<_>>()
    );
}

#[test]
fn cancelled_orders_leave_recoverable_bytes_in_the_arena() {
    // Documenting a real consequence of not zeroing freed slots: the data is
    // unreachable through the tree, but still present in the account. Anything that
    // must actually be erased has to be overwritten explicitly.
    let mut book = Book::new_boxed();
    let id = book
        .place(Side::Bid, Ticks(100), RestingOrder::new(0xDEAD_BEEF, BaseLots(1)))
        .unwrap();
    book.cancel(&id);

    assert!(!book.contains(&id));
    assert_eq!(book.check(), Ok(()));

    let bytes = bytemuck::bytes_of(book.as_ref());
    let needle = 0xDEAD_BEEFu64.to_le_bytes();
    assert!(
        bytes.windows(needle.len()).any(|w| w == needle),
        "expected the cancelled trader index to remain in the arena"
    );
}

#[test]
fn lot_config_survives_a_raw_cast_and_revalidates() {
    let config = LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap();
    let bytes = bytemuck::bytes_of(&config).to_vec();
    let restored: &LotConfig = bytemuck::from_bytes(&bytes);

    assert_eq!(restored, &config);
    assert_eq!(restored.validate(), Ok(()));

    // A cast bypasses the constructor, so corrupt bytes must still be caught.
    let mut corrupt = bytes;
    corrupt[8..16].copy_from_slice(&1_500u64.to_le_bytes());
    let corrupt: &LotConfig = bytemuck::from_bytes(&corrupt);
    assert!(corrupt.validate().is_err(), "revalidation missed a bad tick size");
}
