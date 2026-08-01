import { test } from "node:test";
import assert from "node:assert/strict";

import {
  TAPE_LIMIT,
  bestAsk,
  bestBid,
  depth,
  hasBook,
  initial,
  isFinal,
  mid,
  reduce,
  spread,
} from "../src/lib/feed.ts";
import type { FeedState } from "../src/lib/feed.ts";
import type { WireLevel, WireMessage, WireTrade } from "../src/lib/wire.ts";

function levels(...pairs: [number, number][]): WireLevel[] {
  return pairs.map(([price, size]) => ({
    price_in_ticks: String(price),
    base_lots: String(size),
  }));
}

function trade(slot: number, price: number, size = 1): WireTrade {
  return {
    slot,
    price_in_ticks: String(price),
    base_lots: String(size),
    quote_lots: String(price * size),
    taker_side: "bid",
    maker_seat: 1,
    taker_seat: 2,
    finalized: false,
  };
}

function snapshot(slot: number, finalizedThrough = 0): WireMessage {
  return {
    type: "snapshot",
    market: "M",
    slot,
    finalized_through: finalizedThrough,
    bids: levels([98, 10], [97, 5]),
    asks: levels([102, 7], [103, 9]),
  };
}

function update(slot: number, trades: WireTrade[], finalizedThrough = 0): WireMessage {
  return {
    type: "update",
    slot,
    trades,
    bids: levels([99, 4]),
    asks: levels([101, 6]),
    best_bid: "99",
    best_ask: "101",
    finalized_through: finalizedThrough,
  };
}

function play(messages: WireMessage[], from: FeedState = initial): FeedState {
  return messages.reduce(reduce, from);
}

test("a snapshot replaces the book rather than merging into it", () => {
  const state = play([snapshot(10), update(11, []), snapshot(12)]);

  // The snapshot's two levels a side, not the update's one plus the snapshot's two.
  assert.equal(state.bids.length, 2);
  assert.equal(state.asks.length, 2);
  assert.equal(bestBid(state), 98n);
  assert.equal(state.slot, 12);
  assert.equal(state.status, "live");
});

test("an update replaces the book too", () => {
  // The feed sends the whole top of book on every update, so this is an assignment. If it
  // ever became a merge, a level that emptied would stay on screen forever.
  const state = play([snapshot(10), update(11, [])]);

  assert.equal(state.bids.length, 1);
  assert.equal(bestBid(state), 99n);
  assert.equal(bestAsk(state), 101n);
  assert.equal(spread(state), 2n);
});

test("fills arrive most recent first", () => {
  // A tape is read from the top. The feed sends a transaction's fills in the order the
  // taker consumed them, so the last one it consumed is the most recent.
  const state = play([snapshot(1), update(2, [trade(2, 100), trade(2, 101), trade(2, 102)])]);

  assert.deepEqual(
    state.tape.map((entry) => entry.priceInTicks),
    [102n, 101n, 100n],
  );
});

test("the tape is bounded", () => {
  // Anything older belongs to /history, which is backed by a store. Keeping every fill of
  // a long session is a leak that only appears on the markets worth watching.
  let state = play([snapshot(1)]);
  for (let slot = 2; slot < 2 + TAPE_LIMIT + 40; slot++) {
    state = reduce(state, update(slot, [trade(slot, 100 + slot)]));
  }

  assert.equal(state.tape.length, TAPE_LIMIT);
  assert.equal(state.tape[0]?.slot, 2 + TAPE_LIMIT + 39, "the newest survives");
});

test("a retraction removes the fills from that slot and no others", () => {
  const state = play([
    snapshot(1),
    update(2, [trade(2, 100)]),
    update(3, [trade(3, 101), trade(3, 102)]),
    update(4, [trade(4, 103)]),
    { type: "retract", slot: 3, trades: 2 },
  ]);

  assert.deepEqual(
    state.tape.map((entry) => entry.slot),
    [4, 2],
  );
  assert.equal(state.retracted, 2);
});

test("a retraction counts what it actually removed, not what it was told", () => {
  // A subscriber that connected mid-slot holds fewer fills from it than the server sent.
  // Trusting the count would report retractions that never happened here.
  const state = play([
    snapshot(1),
    update(2, [trade(2, 100)]),
    { type: "retract", slot: 2, trades: 9 },
  ]);

  assert.equal(state.tape.length, 0);
  assert.equal(state.retracted, 1);
});

test("retracting a slot this subscriber never saw changes nothing", () => {
  const state = play([snapshot(1), update(2, [trade(2, 100)]), { type: "retract", slot: 7, trades: 3 }]);

  assert.equal(state.tape.length, 1);
  assert.equal(state.retracted, 0);
});

test("a gap is reported rather than absorbed", () => {
  // Silence and a gap look identical on a quiet market. The server says which it is, and
  // a client that ignores the difference shows a stale book with total confidence.
  const state = play([snapshot(1), { type: "lagged", missed: 12 }]);

  assert.equal(state.status, "gapped");
  assert.equal(state.missed, 12);
});

test("a fresh snapshot is what clears a gap", () => {
  // Not time, and not the next update: the missing changes are missing. Only a whole book
  // replaces a book that is wrong by an unknown amount.
  const gapped = play([snapshot(1), { type: "lagged", missed: 12 }]);
  const recovered = reduce(gapped, snapshot(20, 19));

  assert.equal(recovered.status, "live");
  assert.equal(recovered.slot, 20);
  assert.equal(recovered.missed, 12, "the count is history, not state to reset");
});

test("finality is re-derived rather than trusted from the message", () => {
  // The server stamps `finalized` when it sends. A fill that arrived provisional and has
  // since rooted still carries `false` on the wire object, so reading that flag leaves
  // every trade marked uncertain forever and the flag stops meaning anything.
  const provisional = play([snapshot(1), update(10, [trade(10, 100)], 9)]);
  const entry = provisional.tape[0];
  assert.ok(entry);
  assert.equal(entry.slot, 10);
  assert.equal(isFinal(provisional, entry), false);

  const later = reduce(provisional, update(12, [], 11));
  assert.equal(isFinal(later, entry), true, "slot 10 is rooted once finality passes it");
});

test("an empty side has no price rather than a price of zero", () => {
  const state = reduce(initial, {
    type: "snapshot",
    market: "M",
    slot: 1,
    finalized_through: 0,
    bids: [],
    asks: levels([102, 7]),
  });

  assert.equal(bestBid(state), null);
  assert.equal(bestAsk(state), 102n);
  assert.equal(spread(state), null, "half a market is not a market");
  assert.equal(mid(state), null);
});

test("the midpoint is a tick that exists", () => {
  // Truncating, not rounding to a half. Nothing could have rested at half a tick, and a
  // price no order could hold is a worse answer than a slightly low one.
  const state = reduce(initial, {
    type: "snapshot",
    market: "M",
    slot: 1,
    finalized_through: 0,
    bids: levels([100, 1]),
    asks: levels([103, 1]),
  });

  assert.equal(mid(state), 101n);
});

test("nothing is shown before a snapshot lands", () => {
  assert.equal(hasBook(initial), false);
  assert.equal(initial.status, "connecting");
  assert.equal(hasBook(play([snapshot(1)])), true);
});

test("depth sums a side exactly", () => {
  // bigint throughout: a market's resting size is a u64, and summing in doubles loses
  // lots at exactly the size where the number starts to matter.
  const state = play([snapshot(1)]);
  assert.equal(depth(state.bids), 15n);
  assert.equal(depth(state.asks), 16n);
});

test("reducing never mutates the state it was given", () => {
  // The store hands this state to React, which decides what changed by identity.
  const before = play([snapshot(1), update(2, [trade(2, 100)])]);
  const snapshotOfBefore = JSON.stringify(before, (_, value) =>
    typeof value === "bigint" ? value.toString() : value,
  );

  reduce(before, update(3, [trade(3, 101)]));
  reduce(before, { type: "retract", slot: 2, trades: 1 });
  reduce(before, { type: "lagged", missed: 4 });

  assert.equal(
    JSON.stringify(before, (_, value) => (typeof value === "bigint" ? value.toString() : value)),
    snapshotOfBefore,
  );
});
