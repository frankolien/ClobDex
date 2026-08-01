# clob-mm

A reference market maker. Quotes a two-sided ladder, keeps it there, and takes it down
when it stops.

```
clob-mm --trader alice --reference 151500 --target 49800
```

```
market   Co2FDvpvFr4NWFaLJN2Hb3QxdXkJ7CqxrYYCuNzh8ymY
quoting  96ECLE15x8eSoNGeh5doUh8aphrSthc7jCrAzJKMjaGH
ladder   3 level(s) per side, 100 lots each, 50±20 ticks

fair    151500 [Midpoint]  skew   +0  inv       +0  book 0 -> 4  refresh (a quote is gone)  4DRve…
fair    151500 [Midpoint]  skew   +0  inv       +0  book 4 -> 4  hold
fair    151500 [Midpoint]  skew   +0  inv      -40  book 4 -> 5  refresh (a quote is gone)  5E8TQ…
fair    151500 [Midpoint]  skew   +0  inv      -40  book 5 -> 5  hold
```

## Why this exists

Hand-placed orders make a market that is two-sided for as long as somebody is typing.
Everything measured against that — compute under load, indexer throughput, candles with a
shape — is measured against a market nobody is really making.

It is also the thing that makes `BatchUpdate` worth having. A maker refreshing a ladder
of three levels per side sends one transaction where it would otherwise send twelve.

## The cycle

Four questions, in order, one module each:

| Question | Module |
| --- | --- |
| What is this worth? | `fair` |
| What am I holding, and does that change what I want to quote? | `inventory` |
| So what do I want resting? | `ladder` |
| Is that different enough to be worth a transaction? | `plan` |

All four are pure functions over a decoded market — no sockets, no signing — which is why
the strategy has tests and does not need a cluster to run them. `session` and `main` are
the wiring: read, decide, send, sleep.

## Four decisions worth explaining

**It does not price off the midpoint.** On a market where the bot is the only maker, the
midpoint *is* the bot's own ladder. Quoting around it makes every cycle a fixed point of
the last one, and any rounding at all walks the whole ladder in one direction. So the
touch is read with the bot's own orders removed, and what is left is the part of the book
that is somebody else's opinion. When nothing is left, `--reference` is the opinion.

A book with one side showing is a bound and not a price: a resting bid at 100 says fair is
*at least* 100 and nothing about how much more. The reference supplies the number and the
bound clamps it — which is also what stops a stale reference from quoting an ask
underneath somebody's bid, where post-only would reject it every cycle.

**It cannot cross.** Every quote is post-only and rejected rather than slid, and the
ladder is built so a rejection is not something the strategy can cause: `--max-skew` must
be under `--half-spread`, so no position can push the bid over the ask, and the clamp to
the touch keeps both sides on the right side of everyone else's. A proptest checks it over
the whole parameter space. Sliding would be worse than rejecting — the bot would rest at a
price it did not choose, read that as drift next cycle, and re-quote into the same slide.

**It leans against its position, and then stops.** A maker never chooses what it holds; it
offers both sides and the market picks. Skew shifts the whole ladder toward the side that
unwinds the position, so getting flat is still paid for by the spread. The cap then stops
quoting the side that would deepen it, because a preference does not bound a loss. Both
run off `--inventory-limit`, so the lean reaches its maximum exactly where the cap
engages.

**It holds still.** The ladder is recomputed every cycle and most cycles produce the one
already resting. `--drift` is how far a quote may move before it is worth re-sending; a
refresh also happens whenever a quote is gone or has shrunk, since both mean the bot is
showing less than it believes. A maker's edge is the spread it collects, and fees spent
chasing a tick come out of it.

## Shutting down

`Ctrl-C` cancels both sides before exiting. Quotes outlive the process that placed them,
and an abandoned ladder is still executable at prices nothing is maintaining — the market
gets to trade against a dead bot's last opinion for as long as it stays wrong.

A cycle that fails does not stop the loop. The ladder that was resting is still resting at
prices that were fine a moment ago, and quitting on an RPC hiccup would abandon a live
position over a network error.

## Options

| Flag | Default | |
| --- | --- | --- |
| `--cluster` | `devnet` | which market under `.clob/` |
| `--trader` | payer | quote as a wallet from `clob new-trader` |
| `--reference` | required | what it is worth, in ticks |
| `--half-spread` | 50 | ticks from fair to the first quote |
| `--step` | 25 | ticks between levels |
| `--levels` | 3 | levels per side, at most 12 |
| `--size` | 100 | base lots per level |
| `--target` | 0 | base lots to aim to hold |
| `--inventory-limit` | 5000 | drift from target before a side stops |
| `--max-skew` | 20 | most inventory may shift the ladder |
| `--drift` | 10 | ticks of movement worth re-quoting |
| `--interval` | 5 | seconds between cycles |
| `--once` | | one cycle, then stop |
| `--dry-run` | | decide and print, send nothing |

The ladder is capped at twelve levels per side because a refresh is one transaction:
roughly 72 bytes per level of `BatchUpdate` against 1,232 bytes total. Failing at startup
beats failing on the refresh that mattered.

## Quote it against somebody

Two wallets, or there is nothing to make a market for — a taker that owns the liquidity it
crosses produces a self-trade, no value moves, and no fee is charged.

```
clob new-trader alice
clob --trader alice fund --base 50000 --quote 8000000000
clob-mm --trader alice --reference 151500 --target 50000 &
clob swap bid 151600 40
```
