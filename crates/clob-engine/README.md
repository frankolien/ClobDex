# clob-engine

Crankless matching engine and atomic settlement for on-chain CLOBs.

Built on [`clob-book`](../clob-book), which supplies the data structures. This crate
supplies the policy: order types, self-trade rules, fees, seats, and settlement.
`no_std`, allocation-free, no Solana dependency.

```rust
use clob_engine::{FeeSchedule, Market, OrderPacket, TraderKey};
use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};

let lots = LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap();
let mut market = Market::<32, 32, 8>::new_boxed(lots, FeeSchedule::new(2).unwrap()).unwrap();

let maker = market.claim_seat(TraderKey([1; 32])).unwrap();
let taker = market.claim_seat(TraderKey([2; 32])).unwrap();
market.deposit(maker, BaseLots(100), QuoteLots::ZERO).unwrap();
market.deposit(taker, BaseLots::ZERO, QuoteLots(1_000_000)).unwrap();

market.place_order(maker, OrderPacket::limit(Side::Ask, Ticks(100), BaseLots(10)), &mut ()).unwrap();
let outcome = market.place_order(taker, OrderPacket::market(Side::Bid, BaseLots(4)), &mut ()).unwrap();

assert_eq!(outcome.base_lots_filled, BaseLots(4));
// Settled immediately — the maker can spend this in the next instruction.
assert_eq!(market.traders().state(maker).unwrap().quote_lots_free, QuoteLots(400));
```

## Crankless, and why it works

Serum matched on-chain but could not settle synchronously, so fills went into an event
queue that an off-chain *crank* had to consume. That meant an operational dependency, a
capacity cliff when the queue filled, and proceeds that were not spendable until someone
else ran a process.

Here a fill moves value between two seats' balances inside the taker's transaction. What
makes that safe is that **maker funds are locked at placement**: by the time a resting
order is hit, the value behind it is already committed, so settlement is a transfer that
cannot fail for lack of funds. No queue, no crank, no second step — and no rollback path
needed, which matters because there isn't one mid-instruction.

## Three decisions worth defending

**Fills happen at the maker's price.** The taker's limit decides *whether* a fill
happens, never at what price. A maker who quoted 100 gets 100 even when the taker would
have paid 110. Any other rule makes posting strictly worse than taking, and the book
empties out.

**Resting orders identify their owner by seat index, not public key.** A `SeatIndex` is
the arena handle of that trader's node in the seat table, so settling a fill is an O(1)
array index rather than an O(log n) descent by 32-byte key. Sweeping twenty levels costs
twenty array reads instead of forty tree descents. The price is that a seat can only be
released when it is completely empty — otherwise a resting order would silently
re-target whoever claims the slot next — which `release_seat` enforces.

**Underfunded takers stop rather than getting a clamped partial fill.** Clamping would
make the executed size depend on a fee rounding: an aggregator could not predict it and
a maker could not explain it. Stopping is predictable, and a caller who wants the largest
affordable order can size it before submitting.

## Layout

| Module | Contents |
|---|---|
| [`trader`](src/trader/) | `TraderKey`, `TraderState` (free/locked balances), `TraderTable` (seat allocation). |
| [`order`](src/order.rs) | `OrderPacket`, `SelfTradeBehavior`, `PostOnlyRejection`. |
| [`fees`](src/fees.rs) | `FeeSchedule`. Taker-only, accrued in quote lots, swept separately. |
| [`fill`](src/fill.rs) | `Fill`, `FillObserver`, `OrderOutcome`, `MatchStop`. |
| [`market`](src/market/) | `Market` — the castable value — plus `funds`, `orders` and the `matching` cross loop. |

## The invariant

`Market::check_conservation` states it four ways: seat balances sum to the deposited
totals, locked funds correspond exactly to resting orders, and every order has a live
owner. If a sequence of operations can break it, the market can be drained.

`tests/conservation.rs` asserts it after *every* operation — successful or rejected —
across random operation sequences, alongside an independent external model that tracks
deposits, withdrawals and fee sweeps from outside the engine. That second model is the
point: it catches the case where the engine and its own self-check share a wrong
assumption. Two further properties: the market can always be fully drained (every lot in
comes back out, minus fees), and lifetime fees always reconcile with swept plus unswept.

## One caveat: this crate assumes the caller can revert

`Market::place_order` may mutate the market and *then* return an error — the
fill-or-kill path cannot know it fell short until it has already matched. On Solana that
is free and correct, because a returned error aborts the instruction and every account
write is discarded, which is exactly the all-or-nothing semantic the order type promises.

**Off-chain callers must discard the market on error** rather than continue using it.
Conservation holds either way; what breaks is the expectation that a rejected order
changed nothing.

## Scope

Absent, because they belong to the Solana program above this layer: accounts, CPI, token
transfers, PDAs, events, upgrade authority, and the seat-manager program that makes
`claim_seat` permissionless.

## License

MIT
