# @clobdex/app

The trading application. A static bundle, deployed separately from the marketing site.

```
cp .env.example .env.local     # point it at an indexer
npm run dev
npm test                       # the data layer, without a browser
npm run build                  # static files in dist/
```

## Nothing here is rendered on a server

There is no server half and there could not be one worth having. The book changes every
slot, the wallet exists only in the browser, and a trading screen has no crawler to
satisfy — anything fetched ahead of time is stale before it paints. So this is files on a
CDN, and [`web/`](../web) is a separate deploy for the opposite reason.

Client-side routing means `/trade` and `/portfolio` are not files. `public/_redirects` and
`vercel.json` rewrite everything to `index.html`; a host configured with neither returns
404 on every URL but `/`, and only after deploying, because the dev server falls back on
its own and hides it.

## The hard part is not the pixels

The feed is a state machine, not a resource, and the two messages nobody remembers are the
two that matter:

- **`retract`** — trades already on screen did not happen, because the slot that produced
  them was abandoned. A UI that ignores this displays volume that was never traded.
- **`lagged`** — this subscriber fell behind and the server dropped messages rather than
  stalling ingest for everyone. A gap looks exactly like a quiet market, so it is reported
  and the socket reconnects: only a fresh snapshot fixes a book that is wrong by an unknown
  amount, and no later update carries what was missed.

Neither fires while clicking around a test market. Both fire in production. So the reducer
is a pure function in [`src/lib/feed.ts`](src/lib/feed.ts) with tests for both, and
`node --test` runs them without a DOM.

Finality is re-derived from the current watermark rather than read off each fill. The
server stamps `finalized` when it sends, so a trade that arrived provisional and has since
rooted still says otherwise on the object it came in — trusting that leaves every print
marked uncertain forever and the flag stops meaning anything.

## Every quantity is a `bigint`

A price in ticks is a `u64`, and a JavaScript number stops holding consecutive integers
above 2^53. That is not hypothetical here: a bid's stored sequence number is the complement
of the arrival counter, so it sits just below `u64::MAX`, and a client that read it as a
number would build a cancel for an order that does not exist.

The indexer sends money and identities as strings for that reason.
[`src/lib/wire.ts`](src/lib/wire.ts) says what arrives and
[`src/lib/decode.ts`](src/lib/decode.ts) says what it means, and decoding is strict —
`BigInt("")` is `0n`, and a zero here is a real price that would be rendered as one.

Lot maths comes from the SDK rather than being repeated. It is conformance-tested against
the Rust builders, and a second implementation of a tick conversion is exactly the drift
this repository keeps refusing to introduce.

## Slots are not clocks

The indexer is slot-native throughout, deliberately: block times drift and are occasionally
revised, and a candle whose boundary moves is worse than one measured in an odd unit.
Callers wanting minutes convert at the edge. [`src/lib/time.ts`](src/lib/time.ts) is that
edge and its header says so — fine for axis labels and "3 minutes ago", not fine for
anything anyone would reconcile. Nothing sent to the chain passes through it.

## What it cannot do yet

**Sign anything.** There is no wallet connection, so this is read-only: markets, book,
tape, candles, and any wallet's balances and resting orders by address. Order entry needs
`@solana/kit` and a wallet adapter, and the SDK's instruction builders already return the
shape they take.

**Read mint decimals.** The indexer serves mint addresses, not metadata, so
`ASSUMED_BASE_DECIMALS` and `ASSUMED_QUOTE_DECIMALS` stand in. They match the SOL/USDC
shape the CLI creates; a market with a different one will display shifted. A placeholder,
not a default to rely on.

**Show a portfolio efficiently.** The indexer answers per market, so the portfolio view
asks each one. Fine at devnet scale, wrong at a hundred markets — the fix then is an
endpoint that answers across markets, not a hundred requests from a browser.

## License

MIT
