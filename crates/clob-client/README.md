# clob-client

Client SDK for the ClobDex spot market: typed instruction builders, market account
decoding, and trade event parsing.

```rust
use clob_book::{BaseLots, Side, Ticks};
use clob_client::instruction::{self, MarketAddresses, Receipt};
use clob_client::{MarketState, event};

let addresses = MarketAddresses::new(program_id, market, base_vault, quote_vault);

// A resting quote: two accounts, no event.
let quote = instruction::place_order(
    &addresses,
    &trader,
    &instruction::limit(Side::Bid, Ticks(100), BaseLots(10)),
    Receipt::Off,
);

// Read the book back.
let state = MarketState::decode(&account.data)?;
let bids = state.level_two(Side::Bid, 20);
let cost = state.quote_sweep(Side::Ask, BaseLots(500));

// Read the trade tape out of a transaction.
let fills = event::decode_all(inner_instruction_data)?;
```

## One copy of the wire format

This crate depends on `clob-program` rather than restating discriminants and byte
offsets. **Two copies of a byte layout is how a client and a program drift apart; one
copy cannot.** The cost is that the SDK pulls in the program crate — small, `no_std`,
and it compiles on the host.

The builders are then checked against the program's *own* parser. `tests/wire_format.rs`
decodes every builder's output with `clob_program::instruction::Reader`, including a
property test over every shape of order packet. A builder that writes bytes the program
cannot read fails there rather than on a validator.

The program's own test suite builds its instructions through these builders, so the
on-chain tests exercise the SDK too.

## What it gives you

**Instruction builders** for all eleven instructions, from typed arguments. Vault signer
and log authority bumps are derived here and passed down, because
`find_program_address` costs around a thousand compute units on-chain — doing the search
off-chain is why order entry is 476 CU.

**`MarketState::decode`** turns a market account into owned, size-agnostic data. On-chain
the market is cast in place and never copied because compute is scarce; off-chain an
indexer wants something it can hold, diff and serialise, so this decodes once into plain
`Vec`s rather than handing back a reference into a buffer whose capacities are const
generics.

On top of it: `level_two` (price-level aggregation, with an order count per level —
depth of one large order and depth of twenty small ones behave very differently),
`depth_at_or_better`, `quote_sweep` for price impact without simulating a match, plus
spread and mid.

**`event::decode`** parses a trade receipt out of an inner instruction. Version, kind and
stop code are all validated: a decoder that reads a payload it does not recognise is how
a schema change becomes silent corruption in an index, so unknown values are refused
rather than guessed at.

`OrderPlaced::truncated()` tells a consumer the per-fill tape is incomplete — the
aggregate totals stay exact, so the correct response is to reconstruct the tail from
account diffs rather than record a short trade.

## Receipts are opt-in

`Receipt::On` adds the log authority and program to the account list and a bump byte to
the data. It costs about 1,500 CU. A market maker cancel-replacing continuously already
knows what it submitted; a taker or an aggregator wants the receipt.

## License

MIT
