# clob-indexer

Derives the trade tape and book deltas for a ClobDex market from on-chain state.

This is the half of an indexer that can be tested exhaustively, kept separate from the
half that talks to a gRPC endpoint and a database — because that half cannot.

```rust
let delta = clob_indexer::derive(&before, &after, &instructions, slot);

for trade in &delta.trades {
    // price, size, value, both counterparties — exact, not estimated
}
assert!(delta.fees_reconcile(market.fees().taker_fee_bps));
```

## The tape does not depend on events

Receipts cost ~1,500 CU and are opt-in, so a market maker will never emit one. An
indexer that needed events would see a partial tape — and would not know it.

So the tape is read out of the book itself. An order's **price is part of its identity**
and its size is in the snapshot, so liquidity that disappeared between two states *is*
the tape: exact prices, exact sizes, both counterparties. Receipts become a cross-check
rather than a dependency.

## Why diffing rather than replaying

Replaying the transaction through `clob-engine` is tempting — it's pure and
deterministic, and it reports fills directly. It works for `PlaceOrder` and falls apart
for `Swap`, which deposits and withdraws computed amounts around its match. Replaying
that means reimplementing the program's handler in the indexer, and a second copy of
that logic is exactly the drift this codebase avoids everywhere else.

Diffing duplicates nothing, and `Swap` needs no special case because a swap's fills look
like any other taker's.

## Attribution

The instructions are consulted for one thing: who took, and on which side. Removals are
then classified by:

1. an explicit `CancelOrder`/`ReduceOrder` naming the id → **cancelled**
2. a `CancelAllOrders` covering that side → **cancelled**
3. the side opposite a taker, owned by someone else → **a fill**
4. the side opposite a taker, owned by the taker → **self-traded**
5. anything left → **cancelled**

Rules 3 and 4 are the subtle pair. A self-trade removes liquidity from the opposite side
*exactly* like a fill — that's why it crosses — so side alone cannot separate them; only
the owner can. Getting it wrong would let anyone inflate a market's reported volume by
crossing their own quotes, for free, forever. That's why `ObservedInstruction` carries
the submitting seat, resolved by the caller from the transaction's account keys.

## Two independent checks on the derivation

**Fees.** The market's fee counter is written by the program; the trades here are derived
from the book. `BookDelta::fees_reconcile` compares them. If the derivation invented,
missed, or mispriced a fill, they stop agreeing — a free audit on every fee-charging
market.

**Receipts.** When a taker did pay for one, `cross_check::agrees_with_event` compares two
independent accounts of the same transaction. Disagreement means a wire format change, a
missed instruction, or a bug — found immediately rather than from a user.

## Testing

Ground truth is the engine. Every scenario runs an order through `clob-engine` with a
fill observer attached, so what happened is known exactly; the tape is then derived from
the snapshots alone — with no access to that observer — and compared fill for fill.

That test found a real bug: self-trades were being counted as volume, because the first
version discriminated on side, and side cannot tell a self-trade from a fill.

## Not built yet

The transport. Yellowstone gRPC subscription, slot ordering, gap recovery on reconnect,
commitment reconciliation, and ClickHouse writes — none of which can be verified without
live infrastructure, so none of it is in here yet.

## License

MIT
