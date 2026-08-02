/**
 * The JSON `clob-stream` actually sends, spelled exactly as it arrives.
 *
 * Separate from the decoded types in {@link ./decode.ts} on purpose. These say `string`
 * where the indexer sends a quoted number, because that is what `JSON.parse` produces —
 * writing `bigint` here would describe a value nobody ever holds and move the conversion
 * somewhere it could be forgotten.
 *
 * The indexer's rule, from its README: money and identities are strings, coordinates and
 * tallies are numbers. A `u64` price cannot be a JSON number — a bid's stored sequence
 * number sits just below 2^64, and a double loses it. Slots and seat indices can, being
 * bounded far below 2^53.
 */

/** A price level as sent. */
export interface WireLevel {
  price_in_ticks: string;
  base_lots: string;
}

/** Tick and lot geometry, carried on every market summary. */
export interface WireLots {
  base_lots_per_base_unit: string;
  tick_size_in_quote_lots_per_base_unit: string;
  base_atoms_per_base_lot: string;
  quote_atoms_per_quote_lot: string;
}

/** One market, as `/v1/markets` returns it. */
export interface WireMarketSummary {
  market: string;
  slot: number;
  finalized_through: number;
  base_mint: string;
  quote_mint: string;
  base_vault: string;
  quote_vault: string;
  vault_signer: string;
  taker_fee_bps: number;
  lots: WireLots;
  best_bid_in_ticks: string | null;
  best_ask_in_ticks: string | null;
  spread_in_ticks: string | null;
  mid_price_in_ticks: string | null;
  last_price_in_ticks: string | null;
  bid_orders: number;
  ask_orders: number;
  base_lots_deposited: string;
  quote_lots_deposited: string;
  seats: number;
  trades_seen: number;
}

/** One market's book, as `/book` returns it. */
export interface WireBook {
  market: string;
  slot: number;
  bids: WireLevel[];
  asks: WireLevel[];
  taker_fee_bps: number;
  finalized_through: number;
}

/** A live fill. */
export interface WireTrade {
  slot: number;
  price_in_ticks: string;
  base_lots: string;
  quote_lots: string;
  taker_side: "bid" | "ask";
  maker_seat: number;
  taker_seat: number | null;
  finalized: boolean;
}

/** A stored fill, as `/history` returns it. Always rooted, so it carries no flag. */
export interface WireHistoricalTrade {
  slot: number;
  signature: string;
  price_in_ticks: string;
  base_lots: string;
  quote_lots: string;
  taker_side: "bid" | "ask";
  maker_seat: number;
  taker_seat: number | null;
}

/** Trading activity over a span of slots, as `/window` returns it. */
export interface WireWindow {
  market: string;
  from_slot: number;
  to_slot: number;
  slots: number;
  open_in_ticks: string | null;
  high_in_ticks: string | null;
  low_in_ticks: string | null;
  close_in_ticks: string | null;
  change_in_ticks: string | null;
  vwap_in_ticks: string | null;
  base_lots: string;
  quote_lots: string;
  trades: number;
  truncated: boolean;
}

/** One of a trader's resting orders. */
export interface WireOpenOrder {
  side: "bid" | "ask";
  price_in_ticks: string;
  /** What `CancelOrder` takes. Side-encoded — not the arrival order. */
  order_sequence_number: string;
  /** Arrival order, decoded. What the tape records, so the field to join a fill against. */
  sequence_number: string;
  base_lots: string;
}

/** A trader's position in one market, as `/traders/{wallet}` returns it. */
export interface WireTraderView {
  market: string;
  trader: string;
  seat: number;
  slot: number;
  finalized_through: number;
  base_lots_free: string;
  base_lots_locked: string;
  quote_lots_free: string;
  quote_lots_locked: string;
  orders: WireOpenOrder[];
}

/** One OHLCV bucket, keyed by slot rather than by wall clock. */
export interface WireCandle {
  start_slot: number;
  open: string;
  high: string;
  low: string;
  close: string;
  base_lots: string;
  quote_lots: string;
  trades: number;
}

/**
 * A message from the live feed.
 *
 * Four kinds, and the two that are easy to skip are the two that matter. `retract` says
 * trades already shown did not happen, because the slot that produced them was abandoned.
 * `lagged` says this subscriber fell behind and messages were dropped — silence and a gap
 * look identical, so the feed says which it is.
 */
export type WireMessage =
  | {
      type: "snapshot";
      market: string;
      slot: number;
      finalized_through: number;
      bids: WireLevel[];
      asks: WireLevel[];
    }
  | {
      type: "update";
      slot: number;
      trades: WireTrade[];
      bids: WireLevel[];
      asks: WireLevel[];
      best_bid: string | null;
      best_ask: string | null;
      finalized_through: number;
    }
  | { type: "retract"; slot: number; trades: number }
  | { type: "lagged"; missed: number };
