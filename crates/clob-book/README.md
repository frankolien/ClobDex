# clob-book

Zero-copy limit order book primitives for on-chain CLOBs.

This is the data layer of a Solana central limit order book: the structures a market
account *is*. No matching policy, no settlement, no Solana dependency — `no_std`,
allocation-free, and verified to build for bare-metal targets.

```rust
use clob_book::{BaseLots, OrderBook, RestingOrder, Side, Ticks};

let mut book = OrderBook::<64, 64>::new_boxed();

book.place(Side::Bid, Ticks(100), RestingOrder::new(0, BaseLots(5)));
book.place(Side::Bid, Ticks(101), RestingOrder::new(1, BaseLots(3)));
book.place(Side::Ask, Ticks(103), RestingOrder::new(2, BaseLots(7)));

assert_eq!(book.best_bid().unwrap().key.price_in_ticks, Ticks(101));
assert_eq!(book.spread_in_ticks(), Some(2));
```

## Three design decisions

**Price-time priority is a property of the key type, not of the matching code.**
Bids store a bitwise-inverted sequence number, so a single ascending `Ord` over
`(price, stored_sequence)` gives the correct queue on both sides: the best ask is the
tree minimum, the best bid is the tree maximum, and within a price the oldest order
wins either way. One derive, no side-aware comparator, no second code path. The high
bit of the stored value falls out as a free side tag. See [`order/id.rs`](src/order/id.rs).

**A zeroed account is already a valid empty book.** `NIL` is handle `0` and arena
handles are 1-based, so `SystemProgram::CreateAccount` hands back a usable market with
no initialization pass over an arena that may be hundreds of kilobytes.

**Fills are exact by construction.** `LotConfig::new` rejects any market whose tick size
is not a whole multiple of `base_lots_per_base_unit`. That folds the division in the
fill-value formula into a constant, so no fill ever truncates and no dust accumulates.
The cost is a restricted set of admissible tick sizes; the benefit is that conservation
of funds holds by arithmetic rather than by convention. Phoenix takes the other branch
with a deferred-division intermediate — more flexible, harder to audit.

## Layout

| Module | Contents |
|---|---|
| [`quantities`](src/quantities/) | `Ticks`, `BaseLots`, `QuoteLots`, `BaseAtoms`, `QuoteAtoms`, and `LotConfig`. |
| [`order`](src/order/) | `Side`, `FIFOOrderId` and its encoding, `RestingOrder`. |
| [`tree`](src/tree/) | Fixed-capacity red-black tree over a `Pod` arena, split into `insert` / `remove` / `rotate` / `iter` / `invariants`. |
| [`book`](src/book/) | `OrderBook`: both sides, the sequence counter, and depth queries. |

A market slot is 64 bytes — 16 link bytes, a 16-byte `FIFOOrderId`, a 32-byte
`RestingOrder` — so one node visit is one cache line and tree depth maps directly to
cache misses.

## Testing

```
cargo test                                                    # unit + property tests
cargo build --no-default-features --target thumbv7em-none-eabi  # no_std is real
```

Correctness rests on three things:

- **A differential test against `BTreeMap`.** Random operation sequences run against
  both, with contents, ordering, length, and endpoints compared after *every* operation.
- **A structural self-check after every mutation.** `RedBlackTree::check` verifies the
  red-black properties, parent pointers, ordering, length, and free-list accounting, and
  reports violations as a `Copy` enum with no allocation — so it drops straight into a
  coverage-guided fuzz harness.
- **An independently-sorted priority model.** `tests/book_priority.rs` computes expected
  queue order from raw prices and sequence numbers, so a bug in the bit-inversion
  encoding cannot hide behind a matching bug in the tree.

## Scope

Deliberately absent, because they belong to the engine layer above this one: order types
(`Limit` / `PostOnly` / `IOC`), self-trade behaviour, fees, seats, settlement, and events.
Keeping them out is what makes this core small enough to test exhaustively — and,
eventually, to formally verify.

Compute-unit benchmarks against Phoenix and Manifest need the Solana toolchain and land
with the on-chain program, not here.

## License

MIT
