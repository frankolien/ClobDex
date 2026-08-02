/**
 * Turning what the indexer sent into what the rest of the app holds.
 *
 * One direction only, and one place. Every quantity becomes a `bigint` here and stays one
 * everywhere else, so nothing downstream has to remember which fields were quoted. The
 * SDK's lot maths takes `bigint` for the same reason it exists at all: a price in ticks is
 * a `u64`, and a JavaScript number stops holding consecutive integers at 2^53.
 *
 * Decoding is strict. A quantity that is not a number is an error rather than a `0n`,
 * because a zero here is a real price and a real size, and it would be rendered as one.
 */

import type { LotConfig } from "@clobdex/sdk";

import type {
  WireCandle,
  WireHistoricalTrade,
  WireLevel,
  WireLots,
  WireMarketSummary,
  WireOpenOrder,
  WireTrade,
  WireTraderView,
  WireWindow,
} from "./wire.ts";

/** Thrown when the indexer sends something this cannot read. */
export class DecodeError extends Error {
  constructor(field: string, value: unknown) {
    super(`could not read ${field}: ${JSON.stringify(value)}`);
    this.name = "DecodeError";
  }
}

/**
 * Reads a quoted `u64`.
 *
 * `BigInt("")` is `0n` and `BigInt(" 12 ")` is `12n`, so the input is checked before it is
 * converted rather than after. An empty field silently becoming a zero price is the exact
 * failure this whole module exists to prevent.
 */
export function quantity(value: string, field: string): bigint {
  if (!/^-?\d+$/.test(value)) throw new DecodeError(field, value);
  return BigInt(value);
}

/** The same, for a field the indexer sends as `null` when it has no answer. */
export function maybeQuantity(value: string | null, field: string): bigint | null {
  return value === null ? null : quantity(value, field);
}

export interface Level {
  readonly priceInTicks: bigint;
  readonly baseLots: bigint;
}

export function level(wire: WireLevel): Level {
  return {
    priceInTicks: quantity(wire.price_in_ticks, "price_in_ticks"),
    baseLots: quantity(wire.base_lots, "base_lots"),
  };
}

export function lots(wire: WireLots): LotConfig {
  return {
    baseLotsPerBaseUnit: quantity(wire.base_lots_per_base_unit, "base_lots_per_base_unit"),
    tickSizeInQuoteLotsPerBaseUnit: quantity(
      wire.tick_size_in_quote_lots_per_base_unit,
      "tick_size_in_quote_lots_per_base_unit",
    ),
    baseAtomsPerBaseLot: quantity(wire.base_atoms_per_base_lot, "base_atoms_per_base_lot"),
    quoteAtomsPerQuoteLot: quantity(wire.quote_atoms_per_quote_lot, "quote_atoms_per_quote_lot"),
  };
}

export interface MarketSummary {
  readonly market: string;
  readonly slot: number;
  readonly finalizedThrough: number;
  readonly baseMint: string;
  readonly quoteMint: string;
  readonly baseVault: string;
  readonly quoteVault: string;
  /** The PDA authorising both vaults. Given rather than derived — see the SDK's README. */
  readonly vaultSigner: string;
  readonly takerFeeBps: number;
  readonly lots: LotConfig;
  readonly bestBidInTicks: bigint | null;
  readonly bestAskInTicks: bigint | null;
  readonly spreadInTicks: bigint | null;
  readonly midPriceInTicks: bigint | null;
  readonly lastPriceInTicks: bigint | null;
  readonly bidOrders: number;
  readonly askOrders: number;
  readonly baseLotsDeposited: bigint;
  readonly quoteLotsDeposited: bigint;
  readonly seats: number;
  readonly tradesSeen: number;
}

export function marketSummary(wire: WireMarketSummary): MarketSummary {
  return {
    market: wire.market,
    slot: wire.slot,
    finalizedThrough: wire.finalized_through,
    baseMint: wire.base_mint,
    quoteMint: wire.quote_mint,
    baseVault: wire.base_vault,
    quoteVault: wire.quote_vault,
    vaultSigner: wire.vault_signer,
    takerFeeBps: wire.taker_fee_bps,
    lots: lots(wire.lots),
    bestBidInTicks: maybeQuantity(wire.best_bid_in_ticks, "best_bid_in_ticks"),
    bestAskInTicks: maybeQuantity(wire.best_ask_in_ticks, "best_ask_in_ticks"),
    spreadInTicks: maybeQuantity(wire.spread_in_ticks, "spread_in_ticks"),
    midPriceInTicks: maybeQuantity(wire.mid_price_in_ticks, "mid_price_in_ticks"),
    lastPriceInTicks: maybeQuantity(wire.last_price_in_ticks, "last_price_in_ticks"),
    bidOrders: wire.bid_orders,
    askOrders: wire.ask_orders,
    baseLotsDeposited: quantity(wire.base_lots_deposited, "base_lots_deposited"),
    quoteLotsDeposited: quantity(wire.quote_lots_deposited, "quote_lots_deposited"),
    seats: wire.seats,
    tradesSeen: wire.trades_seen,
  };
}

export interface Fill {
  readonly slot: number;
  readonly priceInTicks: bigint;
  readonly baseLots: bigint;
  readonly quoteLots: bigint;
  readonly takerSide: "bid" | "ask";
  readonly makerSeat: number;
  /** `null` when several takers crossed the same side and the diff could not say which. */
  readonly takerSeat: number | null;
  /** Present only on stored fills; the live feed has no signature to give. */
  readonly signature?: string;
}

export function fill(wire: WireTrade | WireHistoricalTrade): Fill {
  return {
    slot: wire.slot,
    priceInTicks: quantity(wire.price_in_ticks, "price_in_ticks"),
    baseLots: quantity(wire.base_lots, "base_lots"),
    quoteLots: quantity(wire.quote_lots, "quote_lots"),
    takerSide: wire.taker_side,
    makerSeat: wire.maker_seat,
    takerSeat: wire.taker_seat,
    ...("signature" in wire ? { signature: wire.signature } : {}),
  };
}

export interface Window {
  readonly market: string;
  readonly fromSlot: number;
  readonly toSlot: number;
  readonly slots: number;
  readonly openInTicks: bigint | null;
  readonly highInTicks: bigint | null;
  readonly lowInTicks: bigint | null;
  readonly closeInTicks: bigint | null;
  readonly changeInTicks: bigint | null;
  readonly vwapInTicks: bigint | null;
  readonly baseLots: bigint;
  readonly quoteLots: bigint;
  readonly trades: number;
  /** The span held more fills than one query may read, so the totals are a floor. */
  readonly truncated: boolean;
}

export function window(wire: WireWindow): Window {
  return {
    market: wire.market,
    fromSlot: wire.from_slot,
    toSlot: wire.to_slot,
    slots: wire.slots,
    openInTicks: maybeQuantity(wire.open_in_ticks, "open_in_ticks"),
    highInTicks: maybeQuantity(wire.high_in_ticks, "high_in_ticks"),
    lowInTicks: maybeQuantity(wire.low_in_ticks, "low_in_ticks"),
    closeInTicks: maybeQuantity(wire.close_in_ticks, "close_in_ticks"),
    changeInTicks: maybeQuantity(wire.change_in_ticks, "change_in_ticks"),
    vwapInTicks: maybeQuantity(wire.vwap_in_ticks, "vwap_in_ticks"),
    baseLots: quantity(wire.base_lots, "base_lots"),
    quoteLots: quantity(wire.quote_lots, "quote_lots"),
    trades: wire.trades,
    truncated: wire.truncated,
  };
}

export interface OpenOrder {
  readonly side: "bid" | "ask";
  readonly priceInTicks: bigint;
  /**
   * The identity to cancel with, together with the price.
   *
   * Side-encoded: bids store the complement of the arrival counter, which is what makes
   * one ascending comparison price-time priority on both sides. Send this back untouched.
   */
  readonly orderSequenceNumber: bigint;
  /** Arrival order. What a fill records, so this is the one to join a fill against. */
  readonly sequenceNumber: bigint;
  readonly baseLots: bigint;
}

export function openOrder(wire: WireOpenOrder): OpenOrder {
  return {
    side: wire.side,
    priceInTicks: quantity(wire.price_in_ticks, "price_in_ticks"),
    orderSequenceNumber: quantity(wire.order_sequence_number, "order_sequence_number"),
    sequenceNumber: quantity(wire.sequence_number, "sequence_number"),
    baseLots: quantity(wire.base_lots, "base_lots"),
  };
}

export interface Position {
  readonly market: string;
  readonly trader: string;
  readonly seat: number;
  readonly slot: number;
  readonly finalizedThrough: number;
  readonly baseLotsFree: bigint;
  readonly baseLotsLocked: bigint;
  readonly quoteLotsFree: bigint;
  readonly quoteLotsLocked: bigint;
  readonly orders: readonly OpenOrder[];
}

export function position(wire: WireTraderView): Position {
  return {
    market: wire.market,
    trader: wire.trader,
    seat: wire.seat,
    slot: wire.slot,
    finalizedThrough: wire.finalized_through,
    baseLotsFree: quantity(wire.base_lots_free, "base_lots_free"),
    baseLotsLocked: quantity(wire.base_lots_locked, "base_lots_locked"),
    quoteLotsFree: quantity(wire.quote_lots_free, "quote_lots_free"),
    quoteLotsLocked: quantity(wire.quote_lots_locked, "quote_lots_locked"),
    orders: wire.orders.map(openOrder),
  };
}

export interface Candle {
  readonly startSlot: number;
  readonly open: bigint;
  readonly high: bigint;
  readonly low: bigint;
  readonly close: bigint;
  readonly baseLots: bigint;
  readonly quoteLots: bigint;
  readonly trades: number;
}

export function candle(wire: WireCandle): Candle {
  return {
    startSlot: wire.start_slot,
    open: quantity(wire.open, "open"),
    high: quantity(wire.high, "high"),
    low: quantity(wire.low, "low"),
    close: quantity(wire.close, "close"),
    baseLots: quantity(wire.base_lots, "base_lots"),
    quoteLots: quantity(wire.quote_lots, "quote_lots"),
    trades: wire.trades,
  };
}
