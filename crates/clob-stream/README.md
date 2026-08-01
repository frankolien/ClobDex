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

## Not built yet

Persistence. Everything is in memory, so a restart replays from the endpoint's tip and
the tape starts empty. Candles, historical queries, and anything a database is for belong
behind a writer this crate does not have.

## License

MIT
