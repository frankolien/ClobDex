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

## Two accounts to trade

`PlaceOrder`, `CancelOrder`, `ReduceOrder` and `CancelAllOrders` each take exactly two
accounts: the market and the trader's signature. No token accounts, no vaults, no token
program — funds were deposited beforehand and settlement happens inside the market
account.

Aggregators price account count into routing, and two is the floor. The trade-off is
that a taker must pre-fund; an atomic deposit-swap-withdraw for unfunded takers is a
separate instruction that has not been built yet.

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
| 4 | PlaceOrder | market, trader (signer) |
| 5 | CancelOrder | market, trader (signer) |
| 6 | ReduceOrder | market, trader (signer) |
| 7 | CancelAllOrders | market, trader (signer) |
| 8 | CollectFees | market, quote vault, fee recipient, vault signer, token program |

Deposit and withdraw amounts are in **lots, not atoms**. Atoms would mean rounding down
to whole lots and stranding the remainder in the vault, where it would belong to nobody
and break reconciliation between vault balance and recorded deposits.

## Not yet built

Event emission via self-CPI (so indexers read inner-instruction data rather than
truncatable program logs), an atomic swap instruction for unfunded takers, a
permissionless seat-manager program, and market creation from the client side. The
`InitializeMarket` handler validates and writes; allocating the account is still the
client's job.

## License

MIT
