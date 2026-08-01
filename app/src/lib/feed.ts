/**
 * The live feed, as a pure reducer.
 *
 * No socket in this file. The transport belongs somewhere it can be replaced; this is the
 * part that decides what the screen shows, and it is the part that can be wrong in ways
 * nobody notices until a market is busy.
 *
 * Four messages arrive, and the two nobody remembers are the two that matter:
 *
 * - `retract` — trades already displayed did not happen, because the slot that produced
 *   them was abandoned. A UI that ignores this keeps showing prints that never occurred,
 *   and volume that was never traded.
 * - `lagged` — this subscriber fell behind and the server dropped messages for it rather
 *   than stalling ingest for everyone. A gap looks exactly like a quiet market, so the
 *   feed says which it is and the client must resubscribe to close it.
 *
 * Neither fires while clicking around a test market. Both fire in production.
 */

import { type Fill, type Level, fill, level } from "./decode.ts";
import type { WireMessage } from "./wire.ts";

/**
 * How many fills to keep.
 *
 * The tape is a view, not a record: anything older belongs to `/history`, which is backed
 * by a store and can be paged. Holding every fill of a long session is a leak that only
 * shows up on the markets worth watching.
 */
export const TAPE_LIMIT = 250;

/** What the client is getting from the server right now. */
export type Status =
  /** Nothing has arrived yet. */
  | "connecting"
  /** A snapshot has arrived and updates are being applied. */
  | "live"
  /** Messages were dropped. What is shown is stale until a fresh snapshot arrives. */
  | "gapped"
  /** The socket is gone. What is shown is the last thing that was true. */
  | "closed";

export interface FeedState {
  readonly status: Status;
  /** Slot the book came from. Zero before the first message. */
  readonly slot: number;
  /** Everything at or below this is rooted and can no longer be taken back. */
  readonly finalizedThrough: number;
  /** Bids, best first. */
  readonly bids: readonly Level[];
  /** Asks, best first. */
  readonly asks: readonly Level[];
  /** Fills, most recent first, capped at {@link TAPE_LIMIT}. */
  readonly tape: readonly Fill[];
  /** Messages this subscriber never received, cumulative. */
  readonly missed: number;
  /** Fills withdrawn because their slot was abandoned, cumulative. */
  readonly retracted: number;
}

export const initial: FeedState = {
  status: "connecting",
  slot: 0,
  finalizedThrough: 0,
  bids: [],
  asks: [],
  tape: [],
  missed: 0,
  retracted: 0,
};

/**
 * Whether a fill can still be taken back.
 *
 * Derived from the current finality rather than read from the message that carried it.
 * The server stamps `finalized` when it sends, so a fill that arrived provisional and has
 * since rooted still says `false` on the object — trusting it leaves every trade marked
 * uncertain forever, and the flag stops meaning anything.
 */
export function isFinal(state: FeedState, entry: Fill): boolean {
  return entry.slot <= state.finalizedThrough;
}

/** Applies one message. Returns a new state; never mutates the old one. */
export function reduce(state: FeedState, message: WireMessage): FeedState {
  switch (message.type) {
    case "snapshot":
      // A snapshot is the whole truth about the book, so it replaces rather than merges —
      // including after a gap, which is the reason to ask for one.
      return {
        ...state,
        status: "live",
        slot: message.slot,
        finalizedThrough: message.finalized_through,
        bids: message.bids.map(level),
        asks: message.asks.map(level),
      };

    case "update": {
      // The book is sent whole on every update, so it is assigned. Patching levels would
      // be a second implementation of the book, and one that drifts only after some
      // particular sequence of updates is the hardest kind of wrong to find.
      const arrived = message.trades.map(fill);
      return {
        ...state,
        status: "live",
        slot: message.slot,
        finalizedThrough: message.finalized_through,
        bids: message.bids.map(level),
        asks: message.asks.map(level),
        tape: [...arrived].reverse().concat(state.tape).slice(0, TAPE_LIMIT),
      };
    }

    case "retract": {
      // Drop by slot rather than by count. The message says how many went, but a
      // subscriber that joined mid-slot may hold fewer, and removing "the last three"
      // would take fills from a slot that is still perfectly good.
      const kept = state.tape.filter((entry) => entry.slot !== message.slot);
      return {
        ...state,
        tape: kept,
        retracted: state.retracted + (state.tape.length - kept.length),
      };
    }

    case "lagged":
      // Not an error, and not recoverable by waiting. The book on screen is missing an
      // unknown number of changes, so it is marked stale and the caller resubscribes —
      // which produces a fresh snapshot, which is the only thing that fixes it.
      return { ...state, status: "gapped", missed: state.missed + message.missed };
  }
}

/** The book is only meaningful once a snapshot has landed. */
export function hasBook(state: FeedState): boolean {
  return state.status !== "connecting" && (state.bids.length > 0 || state.asks.length > 0);
}

/** Best bid, or `null` on an empty side. */
export function bestBid(state: FeedState): bigint | null {
  return state.bids[0]?.priceInTicks ?? null;
}

/** Best ask, or `null` on an empty side. */
export function bestAsk(state: FeedState): bigint | null {
  return state.asks[0]?.priceInTicks ?? null;
}

/** Ask minus bid, or `null` unless both sides have liquidity. */
export function spread(state: FeedState): bigint | null {
  const bid = bestBid(state);
  const ask = bestAsk(state);
  return bid === null || ask === null ? null : ask - bid;
}

/**
 * Midpoint, or `null` unless both sides have liquidity.
 *
 * Truncating division, so the result is a tick that exists. Half a tick is not a price
 * anything could have rested at.
 */
export function mid(state: FeedState): bigint | null {
  const bid = bestBid(state);
  const ask = bestAsk(state);
  return bid === null || ask === null ? null : (bid + ask) / 2n;
}

/** Total size resting on one side, for a depth bar. */
export function depth(levels: readonly Level[]): bigint {
  return levels.reduce((sum, entry) => sum + entry.baseLots, 0n);
}
