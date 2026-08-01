# clob-stream

Streams ClobDex markets from a Yellowstone endpoint and serves the derived tape.

```
cargo run -p clob-stream          # reads .env
curl localhost:8080/v1/markets
curl localhost:8080/v1/markets/<market>/book?depth=10
curl localhost:8080/v1/markets/<market>/trades
websocat ws://localhost:8080/v1/markets/<market>/stream
```

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

`Memory` is a complete implementation, not a stub, so persistence is optional: without
`CLICKHOUSE_URL` everything still works, just not across a restart.

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

Backfill. The startup snapshot restores current state but not history, so a market's
past is only as deep as whatever this process has seen since it was first pointed at it.

## License

MIT
