# ClobDex

A crankless on-chain order book on Solana. Post, cancel and fill in a single transaction —
no crank, no oracle, no custody. Your funds sit in the market's vaults, your orders settle
atomically inside the taker's transaction, and a market maker refreshes an entire two-sided
ladder for **4,639 compute units**.

> **Devnet, unaudited.** No security review has been done and none is scheduled. Not for
> funds you need back.

```
cargo build-sbf --manifest-path programs/clob/Cargo.toml   # the program
cargo test --workspace                                     # 285 tests, incl. fuzzing
cd crates/clob-stream && cargo run                         # the indexer
cd crates/clob-tui && cargo run -- --market <address>      # watch it from a terminal
cd app && npm run dev                                      # the trading app
cd web && npm run dev                                      # the marketing site
```

## What is here

| | | |
| --- | --- | --- |
| [`programs/clob`](programs/clob) | The program | Native Rust, 13 instructions, red-black trees over `Pod` nodes |
| [`crates/clob-book`](crates/clob-book) | The data structure | Price-time priority from one ascending comparison |
| [`crates/clob-engine`](crates/clob-engine) | Matching | Pure, deterministic, no chain dependency |
| [`crates/clob-client`](crates/clob-client) | Rust SDK | Typed builders, zero-copy state decoding |
| [`crates/clob-indexer`](crates/clob-indexer) | Tape derivation | Reconstructs fills by diffing two book snapshots |
| [`crates/clob-stream`](crates/clob-stream) | The indexer | Yellowstone → REST + WebSocket, with rollback handling |
| [`crates/clob-mm`](crates/clob-mm) | Market maker | Inventory skew, ladders proven not to cross |
| [`crates/clob-cli`](crates/clob-cli) | Operations | Create markets, trade, benchmark |
| [`crates/clob-tui`](crates/clob-tui) | Terminal view | Live book, tape and position — read-only |
| [`ts/sdk`](ts/sdk) | TypeScript SDK | Zero runtime dependencies, held to the Rust bytes |
| [`app`](app) | Trading app | Vite + React, its own deploy |
| [`web`](web) | Marketing site | Astro, its own deploy |

Six Cargo workspaces, not one. Cargo resolves a single dependency graph per workspace
*including dev-dependencies*, and litesvm and solana-client want incompatible versions of
the same crate — so the on-chain crates and each off-chain tool are resolved separately.

The practical consequence is worth knowing before it confuses you: the four excluded
crates are invisible from the root, so `cargo run -p clob-stream` fails with "not found in
workspace". Each is entered and run from its own directory, and `cargo test --workspace`
covers only the on-chain crates — which is why CI runs a job per workspace.

## Why crankless matters

Serum matched in one transaction and settled in another, driven by an off-chain crank. When
the crank stalled, the exchange stalled, and when its event queue filled, it stopped.

Here a taker's order matches and both balances move before the transaction ends. There is
no second step that can fail after the first succeeded, and no process anyone has to keep
running. The indexer reads the chain; it does not stand between you and the program. If it
goes down a UI loses its charts and anyone can still trade by sending instructions.

## Compute, measured

Every figure is from executing the compiled SBF binary under Mollusk. Nothing is estimated.

| instruction | CU |
| --- | ---: |
| Claim a seat | 227 |
| Post-only, empty book | 574 |
| Cancel one order | 844 |
| Market order, one level | 1,500 |
| Batch: 4 cancels, 4 places | 3,251 |
| Market order, four levels | 4,480 |
| Swap, no seat | 6,417 |

Batching four cancels and four places costs 4,639 CU against 6,820 as eight separate
instructions — and one transaction instead of eight, which matters more: eight is eight
signatures, eight slots of exposure, and eight chances to land out of order.

**No comparison against other venues is claimed**, because none has been measured. The
method is published instead, and it needs no cooperation from anyone: `simulateTransaction`
requires no signature that spends anything, so the same procedure runs against any program
on the chain. See [BENCHMARKS.md](BENCHMARKS.md).

## How it is tested

285 tests in the root workspace, and the ones that matter are not unit tests.

**Conserve-funds fuzzing** runs arbitrary sequences of all 13 instructions through the
compiled binary in a real SVM with real SPL vaults, then asserts three things: internal
consistency, that every seat can be wound down to zero, and **vault solvency** — that the
tokens the market says it holds are actually in the vaults. The engine's own property tests
run in memory, where a deposit is a number going up; only this catches a book that balances
while the vault is short.

**The detectors are shown failing.** Fault injectors embezzle and counterfeit, and a test
asserts each fault is caught by exactly one check. A green suite that cannot go red is not
evidence of anything.

**Every commit builds and tests from a clean checkout**, verified nightly by bisecting the
recent history.

## What is not done

- **No audit.** The gating item before mainnet, and not scheduled.
- **No mainnet deployment**, and no date being promised.
- **No global orders.** The capital-efficiency wedge is designed, not built — and it is the
  feature that would make this structurally preferable rather than merely well-tested.
- **Upgrade authority is a single key**, not a multisig.
- **Not routed by any aggregator**, which is the metric that actually decides whether a
  venue matters.
- **The ClickHouse store is unverified against a live server.** Its encoding and parsing are
  tested; its SQL is not. That gap has already produced one real defect.

## License

MIT
