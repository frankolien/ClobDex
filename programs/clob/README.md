# clob-program

The Solana program: a crankless, atomically-settled spot CLOB.

Matching lives in [`clob-engine`](../../crates/clob-engine); this crate is the Solana
surface around it — accounts, instruction encoding, token transfers, and the zero-copy
load that turns a market account into a live market. Written against
[Pinocchio](https://github.com/anza-xyz/pinocchio), `no_std`, no Anchor.

## Measured compute

From Mollusk executing the compiled SBF binary, so these are what a validator charges —
not estimates. Reproduce with:

```
cargo build-sbf --manifest-path programs/clob/Cargo.toml
cargo test -p clob-program --test compute -- --nocapture
```

| Instruction | Accounts | CU |
|---|---:|---:|
| post-only, empty book | 2 | 476 |
| post-only, 16 resting orders | 2 | 684 |
| post-only, 64 resting orders | 2 | 702 |
| market order, 1 level | 2 | 1,359 |
| market order, 4 levels | 2 | 4,228 |
| market order, 16 levels | 2 | 16,528 |
| market order, 64 levels | 2 | 69,388 |
| cancel one order | 2 | 842 |
| cancel 8 orders | 2 | 5,991 |
| single fill, with event receipt | 4 | 2,755 |
| swap, 3 levels, unfunded taker | 8 | 7,135 |

Two things to read off this table. Posting is nearly flat in book depth — 476 CU into an
empty book against 702 into a 64-order book — which is the red-black tree's `O(log n)`
showing up as roughly 220 CU across two doublings. And sweeping is linear at about 1,080
CU per level, so the 1.4M transaction ceiling is not the binding constraint on sweep
depth; `match_limit` is, and the client sets it.

These are *this* program's numbers. Comparing them to Phoenix and Manifest requires
running the same harness against those binaries, which has not been done — Phoenix's
per-instruction CU figures are unpublished, and the "45% lower than Phoenix" figure that
circulates for Manifest is an analyst claim, not a benchmark. Treat the table above as a
baseline to defend, not as a win.

## Events

Program logs are capped per transaction and truncated when they overflow — exactly when
a sweep is deep enough to be worth reading. So a receipt is emitted by calling back into
this program instead, which puts the payload in *inner instruction data*, returned in
full in the transaction meta and not subject to the log budget.

`LogEvent` requires a program-derived signer. Only this program can make its own PDA a
signer, so no user transaction and no other program can forge an event under this
program's id — passing the right address is not enough, it has to actually sign.

**Emission is opt-in.** The receipt costs 1,546 CU and two extra accounts, which more
than triples the cost of a single fill. A market maker cancel-replacing continuously
already knows what it submitted; a taker or aggregator wants the receipt. Appending the
log authority and the program to the account list turns it on, so the cheap path stays
cheap without a second discriminant.

Fills beyond `MAX_LOGGED_FILLS` (24) are dropped from the per-fill detail and a
truncation flag is set. Aggregate totals stay exact either way, so an indexer knows when
it must reconstruct the tail from account diffs rather than having to guess.

The round trip is verified end to end. `tests/event_roundtrip.rs` sends a real signed
transaction through LiteSVM and decodes the event from the transaction's inner
instructions — the same place a Geyser stream or an RPC `getTransaction` response
surfaces it. The parser there is written against the documented field layout rather than
by reusing the encoder, so a change one side makes and the other does not is a test
failure rather than a silently mirrored bug. Swapping two adjacent fields in the encoder
fails two of the six tests.

Mollusk runs the rest of the event tests but cannot do this one: its `inner_instructions`
field sits behind a feature whose dependency does not resolve. Hence two harnesses.

## Two accounts to trade

`PlaceOrder`, `CancelOrder`, `ReduceOrder` and `CancelAllOrders` take two accounts: the
market and the trader's signature. No token accounts, no vaults, no token program —
funds were deposited beforehand and settlement happens inside the market account.
(`PlaceOrder` accepts two more to opt into an event receipt; see above.)

Aggregators price account count into routing, and two is the floor. The trade-off is
that the caller must pre-fund, which is right for a market maker holding inventory on
the venue and wrong for a wallet routed through once — that is what `Swap` is for.

## Swap: trading without a balance

`Swap` deposits, matches and withdraws in one instruction, for callers who hold nothing
on the market. Eight accounts against `PlaceOrder`'s two: the honest price of not
pre-funding.

Three things it gets right that the naive version does not:

**Only the swap's own proceeds leave.** Withdrawing everything free at the end would
drain the standing balance of any trader who is also a maker here. The handler records
the seat's balances before matching and returns the difference — everything, for a
caller with no seat; exactly the trade, for one with inventory.

**The input is computed, not supplied.** The caller names a limit price and a size, and
the program moves in the most that can cost. A caller-supplied amount is one more thing
that can disagree with the order it funds. This is why a swap must be priced: an
unpriced market buy has no bounded cost.

**A seat created by a swap is released again.** Otherwise an aggregator routing
strangers through would fill the trader table with empty seats and eventually lock
everyone out.

## Design notes

**Nothing is deserialized.** A market account is `[header][engine market]` and the market
is cast in place with `bytemuck`. Borsh-decoding a book with thousands of resting orders
would exhaust the compute budget before matching anything.

**Vault addresses live in the header.** Verifying a vault is a 32-byte comparison rather
than a `find_program_address` call, which would otherwise cost around a thousand compute
units on every deposit and withdrawal — more than an entire order placement.

**Size classes, not one size.** Book and seat capacities are const generics, so the
program monomorphises for three classes and `dispatch_market!` turns the runtime class
back into the right static type. One capacity for everything would either price small
markets out with large-market rent or cap large ones at small-market depth.

| Class | Bids / Asks | Seats | Account size |
|---|---:|---:|---:|
| Small | 128 | 32 | 19 KiB |
| Medium | 512 | 128 | 76 KiB |
| Large | 2048 | 512 | 304 KiB |

**Initialization is in place.** `Market::new` builds a market *by value*, which at the
Large class is 606 KiB on a 4 KiB SBF stack. The SBF toolchain caught this; the engine
grew a `Market::initialize` that writes only the configuration into an already-zeroed
account. Worth knowing if you add a code path that constructs a market.

## Instructions

| # | Name | Accounts |
|---:|---|---|
| 0 | InitializeMarket | market, base mint, quote mint, base vault, quote vault, vault signer, authority (signer), fee recipient |
| 1 | ClaimSeat | market, trader (signer) |
| 2 | Deposit | market, trader (signer), trader base, trader quote, base vault, quote vault, token program |
| 3 | Withdraw | + vault signer |
| 4 | PlaceOrder | market, trader (signer), + log authority and program for a receipt |
| 5 | CancelOrder | market, trader (signer) |
| 6 | ReduceOrder | market, trader (signer) |
| 7 | CancelAllOrders | market, trader (signer) |
| 8 | CollectFees | market, quote vault, fee recipient, vault signer, token program |
| 9 | LogEvent | log authority (signer). Emitted by this program, never sent directly |
| 10 | Swap | market, trader (signer), trader base, trader quote, base vault, quote vault, vault signer, token program |

Deposit and withdraw amounts are in **lots, not atoms**. Atoms would mean rounding down
to whole lots and stranding the remainder in the vault, where it would belong to nobody
and break reconciliation between vault balance and recorded deposits.

## Not yet built

A permissionless seat-manager program, and market creation from the client side. The
`InitializeMarket` handler validates and writes; allocating the account is still the
client's job.

## License

MIT
