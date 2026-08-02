# clob-stream

Streams ClobDex markets from a Yellowstone endpoint and serves the derived tape.

```
cargo run -p clob-stream          # reads .env
curl localhost:8080/v1/markets                                  # every market, summarised
curl localhost:8080/v1/markets/<market>/book?depth=10
curl localhost:8080/v1/markets/<market>/trades
curl localhost:8080/v1/markets/<market>/window?slots=216000     # volume, range, change
curl localhost:8080/v1/markets/<market>/traders/<wallet>        # balances, open orders
websocat ws://localhost:8080/v1/markets/<market>/stream
```

## Any origin may read it

Everything here is public on-chain data served over `GET`, with no authentication, no
cookie, no header carrying a secret, and nothing a request can change. An origin
restriction would protect nothing and would only decide who gets to build a client, so the
default is to allow any — the same reason every public market-data API does.

It also has to be permissive to be usable: a browser refuses a cross-origin read without
the header, and a UI on one port talking to an indexer on another is the normal arrangement.
Set `ALLOWED_ORIGINS` to a comma-separated list to narrow it.

**Narrow it the moment anything here needs credentials.** Any origin *plus* credentials is
the combination that turns a read API into a way for any page to act as its visitor.

## Money is a string, coordinates are numbers

Every price, size, value and order identity here is a `u64`. JSON has one numeric type —
an IEEE-754 double — which cannot hold consecutive integers above 2^53, so those fields
cross as decimal strings.

Not a precaution. A bid's stored sequence number is the complement of the arrival counter,
which is what makes one ascending comparison price-time priority on both sides, so it sits
just below `u64::MAX`. The sixth bid ever placed has the identity `18446744073709551610`,
and `JSON.parse` returns `…616`. A client that cancelled with the number it was handed
would cancel nothing and be told nothing.

Slots, seat indices, counts and basis points stay numbers. All are bounded far below 2^53
— slots by roughly a hundred million years of block production — and they are what you
pass back as query parameters, where quoting buys nothing.

An absent price is `null`, never `"0"`. A market with no liquidity has no price, and a
quoted zero parses into a real one.

## What a UI reads

`/v1/markets` returns each market summarised — price, spread, midpoint, depth per side,
what the market holds, the fee, the mints, and the lot geometry needed to format any of
it. Everything comes from memory, so it costs no queries and can be polled.

Rolling volume is deliberately not on it. `/window?slots=N` gives open, high, low, close,
change, VWAP and totals over the last N slots, and costs a store query — folding that into
the list would make the cheapest call the most expensive one. `slots` defaults to 216,000,
about 24 hours at 400ms; that default is the only wall-clock assumption in the crate, and
it selects which trades are counted rather than changing what any of them says. A span
holding more trades than one query may read reports `truncated`, because an under-reported
total is indistinguishable from a real one.

`/traders/<wallet>` is the dashboard: seat, free and locked balances on both sides, and
every resting order. Free and locked are separate because a wallet that deposited and then
quoted still owns all of it while only part can be withdrawn — one combined number matches
neither the vault nor the wallet's arithmetic. `404` when the wallet holds no seat, which
is a different answer from a row of zeroes.

Each open order carries two sequence numbers. `order_sequence_number` is what `CancelOrder`
takes; `sequence_number` is the decoded arrival order, which is what the tape records and
so the field to join a fill against. They are equal on asks and complements on bids, so a
client given only the decoded one works perfectly until it cancels a bid.

Fills name both sides where they can. `taker_seat` is `null` when several takers crossed
the same side in one transaction: a diff sees liquidity leave, not which of them took it,
and naming the first would file one trader's fill under another's.

## The endpoint is the only untestable part

Everything between "an update arrived" and "a delta was derived" runs against a scripted
`Replay` source, with the engine as ground truth: each scenario places orders through
`clob-engine` with a fill observer attached, then feeds the account bytes on either side
through the correlator and pipeline — which cannot see that observer — and compares.

That is why `Source` is a trait. The alternative, testing the pipeline through a live
endpoint, tests the endpoint.

## Correlation

An account update carries what a market became; a transaction update carries what was
asked of it. Derivation needs both, and they arrive separately and in either order, so
one half is held until the other shows up.

Both buffers are bounded and evicted oldest-first. A dropped pairing costs one missing
delta, which the next snapshot corrects; an unbounded buffer costs the process.

## Rollbacks

Indexing at `confirmed` sees a trade about a slot sooner than `finalized` does, and
accepts that the slot can still be abandoned.

So every trade carries whether its slot is rooted, every snapshot carries how far
finality has advanced, and an abandoned slot pushes a `retract` message. A consumer that
cannot tolerate a retraction has the number it needs in order to wait.

Only the tape is corrected. Account state needs no rollback — the writes from a dead slot
were never real, and the next update from a live slot carries the true state. A stale
book self-heals; a phantom trade never would.

`trades_seen` and `trades_retracted` are reported separately rather than netted. Two
published minus one retracted reads identically to one published and nothing wrong.

## Backfill

A restart replays the slots it missed rather than starting at the tip, so an outage
costs latency rather than history. Verified on devnet: killing the process, trading
twice while it was down, and restarting recovered both trades.

That works because the store keeps a **checkpoint** — each market's book at a rooted
slot — alongside the trades. Derivation diffs one book against another, so resuming needs
the book as it stood at the resume point; trades alone are not enough.

The endpoint replays and the same pipeline runs over the replayed slots. A separate
historical decoder would be a second copy of the derivation, which is exactly the drift
this codebase avoids. The limit is whatever history the endpoint retains — beyond that
there is nothing to replay, and no amount of local state changes it.

## Cold starts

A market's first update can only establish a baseline — deriving from it would mean
reporting the whole resting book as newly posted. So on startup every market is fetched
over RPC first, and the first transaction observed after that is already derivable.

Subscribe first, then snapshot. The other order leaves a window in which a transaction
lands between the two and is never seen. Updates older than the snapshot slot are then
ignored, since one still in flight from before it would replace a newer baseline with an
older one.

## Gap recovery

LaserStream tracks the last slot it delivered and replays from there on reconnect. That
is why this uses the Helius client rather than a bare Yellowstone one: without replay,
every disconnect leaves a hole, and a hole in a derived tape is indistinguishable from a
quiet market.

## Two runtimes

actix spawns `!Send` futures per worker; the gRPC client wants a multi-threaded runtime.
Ingest gets its own runtime on its own thread and the two share an `Arc<Registry>`, so a
stalled HTTP worker cannot stop the stream and a stalled stream cannot stop the API from
serving what it already has.

## The fee counter is a free audit

`/health` reports `reconciliation_failures` and returns 500 when it is non-zero. The
market's fee counter is written by the program; the trades are derived independently from
the book. If the two disagree, the derivation invented, missed, or mispriced a fill — and
that is worth alerting on rather than serving quietly.

## Persistence

Nothing is written until its slot is rooted.

That one rule is what makes the store append-only: a retraction can only ever target a
slot still at `confirmed`, and such a trade has not been written yet — so there is never
a row to delete, which a columnar store charges dearly for. The cost is that a trade
becomes durable about a second after it becomes visible, and anyone who wants the faster
answer has the live feed, which says plainly what is still provisional.

Three backends, in order of preference: ClickHouse if `CLICKHOUSE_URL` is set, a
directory if `STORE_PATH` is, memory otherwise. `Memory` is a complete implementation
rather than a fallback stub, so the only thing lost by taking it is durability.

```
curl localhost:8080/v1/markets/<market>/history?from_slot=…&to_slot=…
curl localhost:8080/v1/markets/<market>/candles?interval=150
```

Candles are bucketed by slot rather than wall clock — a slot is what a trade carries,
and block times drift. Aggregation is Rust rather than a SQL `GROUP BY`, because a
rollup would be a second copy of the logic; if it ever becomes the bottleneck, the Rust
version stays as the reference to test it against.

## Not built yet

The ClickHouse store is **unverified against a live server** — no ClickHouse was
reachable when it was written. Its row encoding, parsing and error handling are tested;
its SQL and connection handling are not. `Memory` is fully tested.

That gap has already cost something. `Store::trades` is documented to keep the most recent
rows when a limit bites, which `Memory` did and ClickHouse did not: it ran `ORDER BY slot
ASC LIMIT n` and kept the oldest, so the same call returned the last hundred trades on one
backend and the first hundred ever on the other. Both return `n` rows in ascending order,
so it reads as a quiet market rather than an error. Fixed, and the trait now states which
end a limit takes — but it was found by reading, and only a live server proves the fix.

History older than the endpoint's replay window. A market's past is only as deep as
whatever this process has seen plus whatever the endpoint can still replay; reaching
further would mean deriving from transaction history, which needs a second derivation
path this deliberately does not have.

## License

MIT
