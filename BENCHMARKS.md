# Compute

Compute is the binding constraint on a Solana program and the axis a venue competes on:
it decides how deep a taker can sweep in one transaction, how many quotes a maker can
refresh at once, and whether an aggregator's route through you fits in its budget at all.

Everything here is measured against the compiled SBF binary. Nothing is estimated.

## Method

Two instruments, deliberately.

**Mollusk** executes the real `.so` and reports the compute a validator would charge. It
is the better instrument: exact, deterministic, and able to construct any book depth on
demand. Run it with

```
cargo test -p clob-program --test compute -- --nocapture
```

**`clob bench`** simulates each instruction against a live market with
`simulateTransaction`. It is noisier and it can only measure the book that happens to be
there. It exists for two reasons the first instrument cannot cover: it proves the harness
numbers are what a validator actually charges, and — because a simulation needs no
signature that spends anything and no cooperation from the venue — **the same procedure
can be pointed at somebody else's program.** A compute figure means little if the method
behind it cannot be turned on whatever it is being compared to.

```
clob --cluster devnet --trader alice bench
```

Both report the program's compute. A landed transaction also pays per-signature and
per-account costs that no venue controls, and those are excluded from every number below.

## Measured

Mollusk, `Small` market (128 bids, 128 asks, 32 seats), 2 bps taker fee:

| instruction | accounts | CU |
| --- | ---: | ---: |
| claim a seat | 2 | 227 |
| post-only, empty book | 2 | 574 |
| post-only, 16 resting orders | 2 | 782 |
| post-only, 64 resting orders | 2 | 800 |
| cancel one order | 2 | 844 |
| collect fees | 5 | 1,439 |
| market order, 1 level | 2 | 1,500 |
| batch: 0 cancels, 4 places | 2 | 1,693 |
| limit, crossing 2 levels (seated) | 2 | 2,734 |
| deposit | 7 | 2,753 |
| withdraw | 8 | 2,782 |
| batch: 4 cancels, 4 places | 2 | 3,251 |
| market order, 4 levels | 2 | 4,480 |
| cancel 8 orders | 2 | 5,993 |
| swap, crossing 1 level (no seat) | 8 | 6,417 |
| market order, 16 levels | 2 | 17,224 |

Devnet, same program, live market `Co2FDvpv…zh8ymY` with 6 bids and 6 asks:

| instruction | accounts | CU |
| --- | ---: | ---: |
| claim seat | 2 | 229 |
| post-only, rests | 2 | 905 |
| cancel one | 2 | 990 |
| collect fees | 5 | 1,439 |
| post-only, with receipt | 4 | 2,164 |
| deposit | 7 | 2,780 |
| withdraw | 8 | 2,809 |
| batch: 0 cancels, 4 places | 2 | 3,044 |
| cancel all, up to 8 | 2 | 3,641 |
| batch: 4 cancels, 4 places | 2 | 5,416 |
| swap, 1 level | 8 | 5,627 |
| swap, sweeping 6 levels | 8 | 8,536 |

## What the two tables disagree about, and why

The instructions that do not touch the book agree almost exactly — `collect fees` is
1,439 in both, `claim a seat` is 227 against 229, `deposit` 2,753 against 2,780. That is
the cross-validation: the harness is measuring what the validator measures, and the couple
of CU of difference is per-instruction overhead.

The instructions that *do* touch the book cost more on devnet, and the gap is real rather
than noise. Posting into the live market costs 905 CU against 782 in a fixture book eight
times deeper, so depth is not the explanation. **Seat-table occupancy is.** Every
order-entry instruction begins by resolving a wallet to a seat, and that resolution has a
price:

| seats claimed | post-only CU |
| ---: | ---: |
| 2 | 782 |
| 4 | 809 |
| 8 | 836 |
| 16 | 836 |
| 30 | 863 |

Logarithmic, which is the only acceptable shape — a scan would make trading more expensive
exactly as a market succeeded, and since claiming a seat is cheap and permissionless it
would be a griefing vector as well as a tax. `seat_lookup_does_not_scale_with_the_number_
of_seats` now asserts it.

The general lesson is worth stating plainly: **a fixture with two seats and one owner
understates a live market.** Benchmark numbers taken from an empty book are the most
flattering ones available, and the gap between the two tables above is roughly what that
flattery is worth.

## What the numbers say

**Sweeping is linear, not quadratic.** 1,500 CU for one level, 4,480 for four, 17,224 for
sixteen — about 1,000 CU per level after the first. At the 1.4M limit a taker could cross
far more depth than this book class holds, so the binding constraint on a sweep is the
`match_limit` the caller sets, not the budget.

**Batching is what makes a market maker viable.** Measured against the same book, four
cancels and four places cost **4,639 CU batched against 6,820 CU as eight separate
instructions** — 32% cheaper, and one transaction instead of eight. That second part
matters more than the first: eight transactions is eight signatures, eight slots of
exposure, and eight chances to land in a different order than intended.

Both halves of that comparison come from one fixture, which is why neither matches the
3,251 in the table above — that row is measured in a different world, with a different
book and fewer seats. The same effect as the devnet gap, and the reason a cross-fixture
subtraction is not a result.

**A receipt is a choice, not a default.** Post-only costs 905 CU without one and 2,164
with — the event more than doubles it. A maker refreshing quotes already knows what it
submitted; a taker or an aggregator wants the receipt. Making it optional means the
party that does not need it does not pay for it.

**A seat is worth having if you trade twice.** Crossing two levels costs 2,734 CU with a
seat and 6,417 for the swap that crosses one without one, because a swap settles both legs
through the token program in the same instruction. For an aggregator routing a stranger
once, that is the right trade. For anyone trading repeatedly, deposit once and stop paying
it.

## Comparison with other venues

Not measured here, and the reason is worth being precise about rather than filling the
column with numbers that look authoritative.

**Absolute Phoenix CU figures are unpublished.** The widely cited comparison — that
Manifest's "compute per order is ~45% lower than Phoenix" — comes from Pine Analytics'
*Understanding Manifest*, and it is an analyst's relative claim rather than a benchmark
either team published. Reproducing it means running both programs against live markets
with funded seats, which is a mainnet exercise with real capital and is not something
this repository does on its own authority.

What *is* publicly documented and directly comparable is the account count, because
account count is fixed by the instruction's interface rather than by market conditions:

| venue | accounts to route a swap |
| --- | ---: |
| ClobDex | 8 |
| Phoenix | 8 |
| Manifest | 7 |
| OpenBook v2 | 16 |

That is the honest state of the comparison. `clob bench` is the missing half: it takes a
market address and reports compute without needing the venue's cooperation, so pointing it
at Phoenix or Manifest is a matter of building their instructions and funding a wallet, not
of inventing a methodology. Until someone does that, the numbers in this document describe
one program and claim nothing about any other.

## Reproducing

```
cargo build-sbf --manifest-path programs/clob/Cargo.toml
cargo test -p clob-program --test compute -- --nocapture     # the harness table
clob --cluster devnet --trader alice bench                   # the validator table
```

The harness asserts ceilings rather than exact values, so a change that makes something
cheaper does not fail the suite while a regression does. The ceilings live in
`programs/clob/tests/compute.rs` alongside the measurements.
