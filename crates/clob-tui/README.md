# clob-tui

A market in a terminal.

```
cargo run -- --market <address> --indexer http://localhost:8080 --trader <wallet>
```

```
┌────────────────────────────────────────────────────────────────────────────────┐
│Co2FDvpv…zh8ymY   bid 151.4    ask 151.6    last 151.55    spread 0.2   fee 2 bps│
│2 bids · 2 asks   2 seats · 41 fills seen                                       │
└────────────────────────────────────────────────────────────────────────────────┘
┌ book ────────────────────────┐┌ fills ──────────┐┌ position · seat 1 ──────────┐
│price                    size ││price        size││base free                 4.3│
│151.8                2.6 ▚▚▚▚ ││151.6         0.4││locked                    0.7│
│151.6                0.4 ▚    ││                 ││quote free            8.99902│
│151.5     0.2 spread          ││                 ││locked                0.00098│
│151.4                1.2 ▚▚   ││                 ││                             │
│151.3                3.4 ▚▚▚▚▚││                 ││resting  1                   │
└──────────────────────────────┘└─────────────────┘└─────────────────────────────┘
● live   http://localhost:8080   slot 400121 · 2 from final   q to quit
```

## Read-only, deliberately

No key is loaded and nothing is signed. That is what makes this safe to leave on a screen,
point at somebody else's wallet, or put in a screenshot — none of those are a decision about
custody. Placing an order is `clob-cli`'s job, or the app's.

`--trader` takes any address. Watching a market maker quote is a legitimate thing to want,
and it needs no permission from them.

## Where it gets its numbers

Everything comes from `clob-stream`: the summary and the position over HTTP, the book and
the tape over a WebSocket. The socket URL is derived from the same base as the HTTP calls,
so there is no way to point the feed at a different instance from the one answering the
snapshots — which would show a book and a tape from two processes' idea of one market.

Lot geometry comes from `clob-book`'s `LotConfig`, not from arithmetic written here. It
carries the exactness invariant the program is built on, and it is **validated** rather than
trusted: it arrived over a network from a process that decoded it out of an account, and
every price on screen rests on it. A configuration that fails renders as dashes instead of
as wrong numbers.

## The two messages that matter

A snapshot and an update are exercised by watching any market for a second. These are not,
so they are the ones with tests:

- **`retract`** — fills already on screen did not happen, because the slot that produced
  them was abandoned. They leave the tape entirely rather than being marked; a labelled row
  is still counted by anyone reading the column. The running total is shown once it is
  non-zero.
- **`lagged`** — this subscriber fell behind and the server dropped messages rather than
  stalling ingest for everyone. The book is now wrong by an unknown amount and no later
  update carries what was missed, so the socket closes itself and dials again. Only a fresh
  snapshot fixes it.

Finality is re-derived from the current watermark rather than read off each fill. The server
stamps it when it sends, so a fill that arrived provisional and has since rooted still says
otherwise on the message it came in — trusting that leaves every print dimmed forever and
the marking stops meaning anything.

## Strings on the wire, and why that is not our problem

The indexer quotes money and order identities as decimal strings because JSON has one
numeric type and a `u64` does not fit in a double. A bid's stored sequence number sits just
below `u64::MAX`, and a browser reading it as a number would cancel an order that does not
exist.

Rust parses those back exactly. The constraint is JavaScript's; here it costs one `parse`
per field and nothing else.

## What it does not do

**Trade.** Read-only is the point — see above.

**Read mint decimals.** They are not on the wire, so `--base-decimals` and `--quote-decimals`
default to the SOL/USDC shape the CLI creates. A market with a different shape displays
shifted until told otherwise.

**Chart.** A candle in a terminal is a worse candle. The indexer serves OHLCV if something
wants to.

## Testing

The parts worth testing do not need a terminal:

```
cargo test                       # the reducer, the lot maths, and the layout
cargo test -- --nocapture        # prints the rendered frames
```

Layout is covered with ratatui's `TestBackend`, which renders into a buffer. That catches
the two ways a TUI usually breaks on somebody else's machine — a panel that panics on an
empty state, and one that silently renders nothing — and it already caught depth bars being
clipped off the right edge on every row.
