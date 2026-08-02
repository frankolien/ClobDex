//! The reducer, against the messages that only appear in production.
//!
//! A snapshot and an update are exercised by watching any market for a second. Retraction
//! and lag are not — they need a reorg and a slow consumer — so they are the two worth
//! testing, and the two most likely to be wrong.

use clob_tui::feed::{Action, Feed};
use clob_tui::wire;

fn level(price: u64, size: u64) -> wire::Level {
    wire::Level {
        price_in_ticks: price.to_string(),
        base_lots: size.to_string(),
    }
}

fn trade(slot: u64, price: u64, size: u64) -> wire::Trade {
    wire::Trade {
        slot,
        price_in_ticks: price.to_string(),
        base_lots: size.to_string(),
        taker_side: "bid".into(),
        maker_seat: 1,
        taker_seat: Some(2),
    }
}

fn snapshot(slot: u64) -> wire::Message {
    wire::Message::Snapshot {
        slot,
        finalized_through: slot.saturating_sub(2),
        bids: vec![level(98, 10), level(97, 5)],
        asks: vec![level(102, 7)],
    }
}

fn update(slot: u64, trades: Vec<wire::Trade>) -> wire::Message {
    wire::Message::Update {
        slot,
        trades,
        bids: vec![level(98, 4)],
        asks: vec![level(102, 7)],
        finalized_through: slot.saturating_sub(2),
    }
}

#[test]
fn a_snapshot_is_the_whole_book() {
    let mut feed = Feed::default();
    assert_eq!(feed.apply(snapshot(100)), Action::None);

    assert!(feed.ready);
    assert_eq!(feed.best_bid(), Some(98));
    assert_eq!(feed.best_ask(), Some(102));
    assert_eq!(feed.spread(), Some(4));
    assert_eq!(feed.mid(), Some(100));
}

#[test]
fn an_update_replaces_the_book_rather_than_patching_it() {
    // The feed sends the whole top of book. Applying it as a diff would be a second
    // implementation of the book, and one that only goes wrong after some unpredictable
    // sequence is the hardest kind to notice.
    let mut feed = Feed::default();
    feed.apply(snapshot(100));
    assert_eq!(feed.bids.len(), 2);

    feed.apply(update(101, vec![]));
    assert_eq!(feed.bids.len(), 1, "the level that left is gone, not stale");
    assert_eq!(feed.bids[0].base_lots, 4);
    assert_eq!(feed.slot, 101);
}

#[test]
fn a_retracted_slot_leaves_the_tape_entirely() {
    // The slot was abandoned, so these fills did not happen. Marking them instead of
    // removing them would still have them counted by anyone reading the column.
    let mut feed = Feed::default();
    feed.apply(snapshot(100));
    feed.apply(update(101, vec![trade(101, 100, 5), trade(101, 99, 3)]));
    feed.apply(update(102, vec![trade(102, 101, 2)]));
    assert_eq!(feed.tape.len(), 3);

    feed.apply(wire::Message::Retract {
        slot: 101,
        trades: 2,
    });

    assert_eq!(feed.tape.len(), 1, "only slot 102 survives");
    assert_eq!(feed.tape[0].slot, 102);
    assert_eq!(feed.retracted, 2);
}

#[test]
fn retracting_a_slot_that_was_never_shown_is_harmless() {
    // A retraction can arrive for a slot whose fills were never sent to this subscriber.
    // It must not remove anything else, and it must not panic on the empty difference.
    let mut feed = Feed::default();
    feed.apply(snapshot(100));
    feed.apply(update(101, vec![trade(101, 100, 5)]));

    feed.apply(wire::Message::Retract {
        slot: 999,
        trades: 4,
    });

    assert_eq!(feed.tape.len(), 1, "nothing else was touched");
    assert_eq!(feed.retracted, 4, "the server's count is still recorded");
}

#[test]
fn falling_behind_asks_for_a_new_snapshot() {
    // The book is now wrong by an unknown amount and no later update carries what was
    // dropped. Reconnecting is the only thing that fixes it.
    let mut feed = Feed::default();
    feed.apply(snapshot(100));
    assert!(feed.ready);

    assert_eq!(
        feed.apply(wire::Message::Lagged { missed: 12 }),
        Action::Resubscribe
    );
    assert!(!feed.ready, "the book is no longer trustworthy");
    assert_eq!(feed.missed, 12);
}

#[test]
fn finality_is_read_from_the_watermark_not_from_the_fill() {
    // The server stamps finality when it sends, so a fill that arrived provisional and has
    // since rooted still says otherwise on the message it came in. Trusting that leaves
    // every print marked uncertain forever and the flag stops meaning anything.
    let mut feed = Feed::default();
    feed.apply(snapshot(100));
    feed.apply(update(101, vec![trade(101, 100, 5)]));

    let fill = feed.tape[0].clone();
    assert!(!feed.is_final(&fill), "slot 101, rooted through 99");

    feed.apply(update(105, vec![]));
    assert!(feed.is_final(&fill), "rooted through 103 now");
}

#[test]
fn a_one_sided_book_reports_no_spread_and_no_midpoint() {
    // Half a market is not a market. A midpoint from one side would be invented, and a
    // number nobody quoted is worse than a blank.
    let mut feed = Feed::default();
    feed.apply(wire::Message::Snapshot {
        slot: 1,
        finalized_through: 0,
        bids: vec![level(98, 10)],
        asks: vec![],
    });

    assert_eq!(feed.best_bid(), Some(98));
    assert_eq!(feed.best_ask(), None);
    assert_eq!(feed.spread(), None);
    assert_eq!(feed.mid(), None);
}

#[test]
fn a_malformed_quantity_costs_a_level_not_the_session() {
    // One unreadable frame should not take down a viewer that is otherwise correct.
    let mut feed = Feed::default();
    feed.apply(wire::Message::Snapshot {
        slot: 1,
        finalized_through: 0,
        bids: vec![
            level(98, 10),
            wire::Level {
                price_in_ticks: "not a number".into(),
                base_lots: "5".into(),
            },
        ],
        asks: vec![],
    });

    assert_eq!(feed.bids.len(), 1);
    assert_eq!(feed.best_bid(), Some(98));
}

#[test]
fn the_tape_is_newest_first_and_bounded() {
    let mut feed = Feed::default();
    feed.apply(snapshot(1));
    for slot in 2..400 {
        feed.apply(update(slot, vec![trade(slot, 100 + slot, 1)]));
    }

    assert_eq!(feed.tape.len(), 256, "bounded, so a busy market cannot grow it");
    assert_eq!(feed.tape[0].slot, 399, "newest first");
    assert!(feed.tape[0].slot > feed.tape[1].slot);
}
