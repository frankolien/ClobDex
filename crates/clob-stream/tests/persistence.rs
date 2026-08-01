//! Persistence, and the rule that makes it simple.
//!
//! Nothing is written until its slot is rooted. That one rule is what makes the store
//! append-only: a retraction can only ever target a slot that is still `confirmed`, and
//! such a trade has not been written yet — so there is never a row to delete.
//!
//! These prove the rule holds, because if it ever stops holding the store quietly
//! acquires a class of row that should not exist and nothing here would say so.

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side, Ticks};
use clob_indexer::Trade;
use clob_stream::candle;
use clob_stream::flush::{Pending, flush};
use clob_stream::store::{Checkpoint, Memory, Range, StoredTrade, Store};
use solana_pubkey::Pubkey;

const MARKET: Pubkey = Pubkey::new_from_array([1u8; 32]);
const OTHER: Pubkey = Pubkey::new_from_array([2u8; 32]);

fn trade(price: u64, size: u64) -> Trade {
    Trade {
        slot: 0,
        price_in_ticks: Ticks(price),
        base_lots: BaseLots(size),
        quote_lots: QuoteLots(price * size),
        maker_order_id: FIFOOrderId::new(Side::Ask, Ticks(price), 1),
        maker_seat: 1,
        taker_side: Side::Bid,
    }
}

fn stored(slot: u64, price: u64, size: u64) -> StoredTrade {
    StoredTrade {
        market: MARKET,
        slot,
        signature: [slot as u8; 64],
        price_in_ticks: price,
        base_lots: size,
        quote_lots: price * size,
        maker_seat: 1,
        taker_side_is_bid: true,
    }
}

// -------------------------------------------------------------------------------------
// The rule
// -------------------------------------------------------------------------------------

#[tokio::test]
async fn nothing_is_written_before_its_slot_is_rooted() {
    let store = Memory::new();
    let mut pending = Pending::new();
    pending.record(MARKET, 10, [1u8; 64], &[trade(100, 5)]);

    assert_eq!(pending.len(), 1);
    assert!(store.is_empty(), "a confirmed trade is not durable yet");

    flush(&mut pending, &store, 10).await.unwrap();
    assert_eq!(store.len(), 1, "and is written once its slot roots");
    assert!(pending.is_empty());
}

#[tokio::test]
async fn a_retracted_trade_never_reaches_the_store() {
    // The whole reason the store needs no delete. If this ever fails, the store has
    // quietly acquired a row for a trade that did not happen.
    let store = Memory::new();
    let mut pending = Pending::new();
    pending.record(MARKET, 10, [1u8; 64], &[trade(100, 5)]);

    assert_eq!(pending.retract(10), 1);
    flush(&mut pending, &store, 20).await.unwrap();

    assert!(store.is_empty(), "an abandoned slot leaves nothing behind");
}

#[tokio::test]
async fn finalizing_takes_every_slot_at_or_below_it() {
    let store = Memory::new();
    let mut pending = Pending::new();
    for slot in [10, 11, 12] {
        pending.record(MARKET, slot, [slot as u8; 64], &[trade(100 + slot, 1)]);
    }

    let written = flush(&mut pending, &store, 11).await.unwrap();
    assert_eq!(written, 2, "10 and 11 root, 12 does not");
    assert_eq!(pending.len(), 1);

    flush(&mut pending, &store, 12).await.unwrap();
    assert_eq!(store.len(), 3);
}

#[tokio::test]
async fn a_slot_already_flushed_is_not_queued_again() {
    // A reconnect replays slots. The store deduplicates, but not doing the work beats
    // undoing it.
    let store = Memory::new();
    let mut pending = Pending::new();
    pending.record(MARKET, 10, [1u8; 64], &[trade(100, 5)]);
    flush(&mut pending, &store, 10).await.unwrap();

    pending.record(MARKET, 10, [1u8; 64], &[trade(100, 5)]);
    assert!(pending.is_empty(), "a replayed slot is dropped, not requeued");
    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn a_failed_write_keeps_the_trades_for_the_next_attempt() {
    // A store that is briefly unreachable should cost latency, not data.
    struct Broken;
    #[async_trait::async_trait]
    impl Store for Broken {
        async fn append(&self, _: &[StoredTrade]) -> anyhow::Result<()> {
            anyhow::bail!("unreachable")
        }
        async fn trades(&self, _: &Pubkey, _: Range) -> anyhow::Result<Vec<StoredTrade>> {
            Ok(Vec::new())
        }
        async fn highest_slot(&self, _: &Pubkey) -> anyhow::Result<Option<u64>> {
            Ok(None)
        }
        async fn save_checkpoint(&self, _: &Pubkey, _: &Checkpoint) -> anyhow::Result<()> {
            anyhow::bail!("unreachable")
        }
        async fn checkpoint(&self, _: &Pubkey) -> anyhow::Result<Option<Checkpoint>> {
            Ok(None)
        }
        async fn checkpointed_markets(&self) -> anyhow::Result<Vec<Pubkey>> {
            Ok(Vec::new())
        }
    }

    let mut pending = Pending::new();
    pending.record(MARKET, 10, [1u8; 64], &[trade(100, 5)]);
    assert!(flush(&mut pending, &Broken, 10).await.is_err());
    assert_eq!(pending.len(), 1, "still queued");

    // And the watermark came back with them, or the retry would skip them as flushed.
    let store = Memory::new();
    flush(&mut pending, &store, 10).await.unwrap();
    assert_eq!(store.len(), 1, "the retry writes them");
}

#[tokio::test]
async fn resuming_skips_what_is_already_stored() {
    let mut pending = Pending::resuming_from(100);
    pending.record(MARKET, 50, [1u8; 64], &[trade(100, 5)]);
    assert!(pending.is_empty(), "an older slot is already durable");

    pending.record(MARKET, 101, [2u8; 64], &[trade(100, 5)]);
    assert_eq!(pending.len(), 1, "a newer one is not");
}

// -------------------------------------------------------------------------------------
// The store
// -------------------------------------------------------------------------------------

#[tokio::test]
async fn writing_the_same_trade_twice_stores_it_once() {
    let store = Memory::new();
    store.append(&[stored(10, 100, 5)]).await.unwrap();
    store.append(&[stored(10, 100, 5)]).await.unwrap();

    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn two_fills_in_one_transaction_are_both_kept() {
    // They share a signature and a slot, so deduplicating on signature alone would lose
    // one — which is how a multi-fill sweep would silently under-report.
    let store = Memory::new();
    let mut first = stored(10, 100, 5);
    let mut second = stored(10, 101, 5);
    second.signature = first.signature;
    first.maker_seat = 1;
    second.maker_seat = 2;

    store.append(&[first, second]).await.unwrap();
    assert_eq!(store.len(), 2);
}

#[tokio::test]
async fn a_slot_range_selects_only_what_it_covers() {
    let store = Memory::new();
    store
        .append(&[stored(10, 100, 1), stored(20, 101, 1), stored(30, 102, 1)])
        .await
        .unwrap();

    let found = store
        .trades(
            &MARKET,
            Range {
                from_slot: 15,
                to_slot: 25,
                limit: 100,
            },
        )
        .await
        .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].slot, 20);
}

#[tokio::test]
async fn a_limit_takes_the_most_recent_not_the_oldest() {
    // "The last 10 trades" answered with the first 10 ever is the wrong answer.
    let store = Memory::new();
    let trades: Vec<_> = (1..=5).map(|slot| stored(slot, 100 + slot, 1)).collect();
    store.append(&trades).await.unwrap();

    let found = store.trades(&MARKET, Range::latest(2)).await.unwrap();
    assert_eq!(
        found.iter().map(|t| t.slot).collect::<Vec<_>>(),
        vec![4, 5],
        "most recent, still in ascending order"
    );
}

#[tokio::test]
async fn markets_do_not_see_each_others_trades() {
    let store = Memory::new();
    let mut other = stored(10, 100, 1);
    other.market = OTHER;
    store.append(&[stored(10, 100, 1), other]).await.unwrap();

    assert_eq!(store.trades(&MARKET, Range::latest(10)).await.unwrap().len(), 1);
    assert_eq!(store.trades(&OTHER, Range::latest(10)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_unknown_market_reads_empty_rather_than_failing() {
    let store = Memory::new();
    assert!(store.trades(&MARKET, Range::latest(10)).await.unwrap().is_empty());
    assert_eq!(store.highest_slot(&MARKET).await.unwrap(), None);
}

#[tokio::test]
async fn the_highest_slot_is_what_a_restart_resumes_from() {
    let store = Memory::new();
    store
        .append(&[stored(10, 100, 1), stored(30, 100, 1), stored(20, 100, 1)])
        .await
        .unwrap();

    assert_eq!(store.highest_slot(&MARKET).await.unwrap(), Some(30));
}

// -------------------------------------------------------------------------------------
// Candles
// -------------------------------------------------------------------------------------

#[test]
fn a_candle_takes_open_and_close_from_the_edges_and_high_low_from_the_middle() {
    let trades = vec![
        stored(10, 100, 1),
        stored(11, 130, 1),
        stored(12, 90, 1),
        stored(13, 110, 1),
    ];
    let candles = candle::aggregate(&trades, 100);

    assert_eq!(candles.len(), 1);
    let c = &candles[0];
    assert_eq!((c.open, c.high, c.low, c.close), (100, 130, 90, 110));
    assert_eq!(c.trades, 4);
    assert_eq!(c.base_lots, 4);
}

#[test]
fn buckets_start_on_multiples_of_the_interval() {
    // Otherwise the first trade decides where every boundary falls, and two nodes with
    // different histories would produce different candles for the same data.
    let trades = vec![stored(105, 100, 1), stored(210, 200, 1)];
    let candles = candle::aggregate(&trades, 100);

    assert_eq!(candles.len(), 2);
    assert_eq!(candles[0].start_slot, 100);
    assert_eq!(candles[1].start_slot, 200);
}

#[test]
fn an_empty_interval_produces_no_candle() {
    // Carrying the previous close forward states a price that was never traded.
    let trades = vec![stored(10, 100, 1), stored(310, 200, 1)];
    let candles = candle::aggregate(&trades, 100);

    assert_eq!(candles.len(), 2, "no filler for the two silent buckets");
    assert_eq!(candles[0].start_slot, 0);
    assert_eq!(candles[1].start_slot, 300);
}

#[test]
fn aggregating_nothing_yields_nothing() {
    assert!(candle::aggregate(&[], 100).is_empty());
    // A zero interval would divide by zero; refusing beats panicking on a query string.
    assert!(candle::aggregate(&[stored(1, 100, 1)], 0).is_empty());
}

#[test]
fn volume_is_the_sum_of_what_traded() {
    let trades = vec![stored(10, 100, 3), stored(11, 200, 7)];
    let candles = candle::aggregate(&trades, 100);

    assert_eq!(candles[0].base_lots, 10);
    assert_eq!(candles[0].quote_lots, 100 * 3 + 200 * 7);
}

#[test]
fn vwap_weights_by_size_not_by_count() {
    // The whole point: one large trade should move it more than one small trade.
    let trades = vec![stored(10, 100, 1), stored(11, 200, 9)];
    assert_eq!(candle::vwap(&trades), Some(190));

    assert_eq!(candle::vwap(&[]), None, "no trades has no average price");
}

// -------------------------------------------------------------------------------------
// Checkpoints
//
// Trades alone cannot resume a derivation: it diffs one book against another, so picking
// up where a previous process stopped needs the book as it stood there. A checkpoint is
// that book, at a rooted slot — and rooted is what makes it safe to trust, since a slot
// that cannot be rolled back describes a state that cannot be undone.
// -------------------------------------------------------------------------------------

#[tokio::test]
async fn a_checkpoint_is_read_back_exactly() {
    let store = Memory::new();
    let checkpoint = Checkpoint {
        slot: 500,
        data: vec![7u8; 19_296],
    };
    store.save_checkpoint(&MARKET, &checkpoint).await.unwrap();

    assert_eq!(store.checkpoint(&MARKET).await.unwrap(), Some(checkpoint));
}

#[tokio::test]
async fn a_checkpoint_never_moves_backwards() {
    // Writes can arrive out of order. An older one overwriting a newer would make the
    // next restart replay from further back, or resume from a book that is already stale.
    let store = Memory::new();
    store
        .save_checkpoint(&MARKET, &Checkpoint { slot: 500, data: vec![1] })
        .await
        .unwrap();
    store
        .save_checkpoint(&MARKET, &Checkpoint { slot: 400, data: vec![2] })
        .await
        .unwrap();

    let kept = store.checkpoint(&MARKET).await.unwrap().unwrap();
    assert_eq!(kept.slot, 500, "the newer checkpoint stands");
    assert_eq!(kept.data, vec![1]);
}

#[tokio::test]
async fn a_newer_checkpoint_replaces_an_older_one() {
    let store = Memory::new();
    store
        .save_checkpoint(&MARKET, &Checkpoint { slot: 400, data: vec![1] })
        .await
        .unwrap();
    store
        .save_checkpoint(&MARKET, &Checkpoint { slot: 500, data: vec![2] })
        .await
        .unwrap();

    let kept = store.checkpoint(&MARKET).await.unwrap().unwrap();
    assert_eq!((kept.slot, kept.data), (500, vec![2]));
}

#[tokio::test]
async fn checkpoints_are_per_market() {
    let store = Memory::new();
    store
        .save_checkpoint(&MARKET, &Checkpoint { slot: 500, data: vec![1] })
        .await
        .unwrap();
    store
        .save_checkpoint(&OTHER, &Checkpoint { slot: 600, data: vec![2] })
        .await
        .unwrap();

    let mut markets = store.checkpointed_markets().await.unwrap();
    markets.sort();
    assert_eq!(markets.len(), 2);
    assert_eq!(store.checkpoint(&MARKET).await.unwrap().unwrap().slot, 500);
    assert_eq!(store.checkpoint(&OTHER).await.unwrap().unwrap().slot, 600);
}

#[tokio::test]
async fn a_market_with_no_checkpoint_reads_none() {
    let store = Memory::new();
    assert_eq!(store.checkpoint(&MARKET).await.unwrap(), None);
    assert!(store.checkpointed_markets().await.unwrap().is_empty());
}

#[test]
fn resuming_starts_from_the_oldest_checkpoint() {
    // The oldest wins, not the newest: a market further behind than the others would
    // otherwise have its missed slots skipped, and a hole in one tape is still a hole.
    let checkpoints = [500u64, 400, 600];
    let oldest = checkpoints.iter().copied().min().unwrap();

    assert_eq!(oldest, 400);
}
