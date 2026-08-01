/**
 * The indexer's REST surface, typed.
 *
 * Every method decodes before returning, so nothing above this line ever holds a wire
 * shape. The split is deliberate: {@link ./wire.ts} says what arrives, {@link ./decode.ts}
 * says what it means, and this file is the only place a URL is written down.
 *
 * A failed request throws. Returning an empty list on a dead backend would make "the
 * indexer is down" indistinguishable from "nothing has traded", which is the same mistake
 * the indexer itself refuses to make when its store is unreachable.
 */

import {
  type Candle,
  type Fill,
  type MarketSummary,
  type Position,
  type Window,
  candle,
  fill,
  level,
  marketSummary,
  position,
  window,
} from "./decode.ts";
import type {
  WireBook,
  WireCandle,
  WireHistoricalTrade,
  WireMarketSummary,
  WireTraderView,
  WireWindow,
} from "./wire.ts";

/** A request the indexer refused or could not answer. */
export class IndexerError extends Error {
  // Declared and assigned rather than written as constructor parameter properties: those
  // emit runtime code, which `erasableSyntaxOnly` forbids so that Node can run this source
  // by stripping types instead of compiling it.
  readonly status: number;
  readonly path: string;

  constructor(status: number, path: string, body: string) {
    super(`${path} returned ${status}: ${body || "no body"}`);
    this.name = "IndexerError";
    this.status = status;
    this.path = path;
  }
}

/** A book at a slot, decoded. */
export interface Book {
  readonly market: string;
  readonly slot: number;
  readonly finalizedThrough: number;
  readonly takerFeeBps: number;
  readonly bids: ReturnType<typeof level>[];
  readonly asks: ReturnType<typeof level>[];
}

export class Indexer {
  private readonly baseUrl: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  private async get<T>(path: string, signal?: AbortSignal): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      signal: signal ?? null,
      headers: { accept: "application/json" },
    });
    if (!response.ok) {
      throw new IndexerError(response.status, path, await response.text().catch(() => ""));
    }
    return (await response.json()) as T;
  }

  /**
   * The same, for a route where absence is an answer.
   *
   * Only 404 becomes `null`. A 503 still throws, because "this wallet has no seat" and
   * "the indexer cannot tell you" are different facts and only one of them means the UI
   * should offer to claim a seat.
   */
  private async find<T>(path: string, signal?: AbortSignal): Promise<T | null> {
    try {
      return await this.get<T>(path, signal);
    } catch (error) {
      if (error instanceof IndexerError && error.status === 404) return null;
      throw error;
    }
  }

  /** Every tracked market, summarised. Cheap enough to poll: the indexer serves it from memory. */
  async markets(signal?: AbortSignal): Promise<MarketSummary[]> {
    const wire = await this.get<WireMarketSummary[]>("/v1/markets", signal);
    return wire.map(marketSummary);
  }

  /** One market's book, deeper than the live feed carries. */
  async book(market: string, depth = 20, signal?: AbortSignal): Promise<Book> {
    const wire = await this.get<WireBook>(`/v1/markets/${market}/book?depth=${depth}`, signal);
    return {
      market: wire.market,
      slot: wire.slot,
      finalizedThrough: wire.finalized_through,
      takerFeeBps: wire.taker_fee_bps,
      bids: wire.bids.map(level),
      asks: wire.asks.map(level),
    };
  }

  /**
   * Volume, range and change over a span of slots.
   *
   * `slots` defaults to the indexer's own default rather than being restated here — one
   * definition of "24 hours" across the two, so a label cannot describe a different span
   * from the one that was measured.
   */
  async window(market: string, slots?: number, signal?: AbortSignal): Promise<Window> {
    const query = slots === undefined ? "" : `?slots=${slots}`;
    return window(await this.get<WireWindow>(`/v1/markets/${market}/window${query}`, signal));
  }

  /** A wallet's balances and resting orders, or `null` if it holds no seat here. */
  async position(market: string, trader: string, signal?: AbortSignal): Promise<Position | null> {
    const wire = await this.find<WireTraderView>(
      `/v1/markets/${market}/traders/${trader}`,
      signal,
    );
    return wire === null ? null : position(wire);
  }

  /** Stored fills, oldest first. Bounded by the indexer whatever is asked for. */
  async history(
    market: string,
    options: { fromSlot?: number; toSlot?: number; limit?: number } = {},
    signal?: AbortSignal,
  ): Promise<Fill[]> {
    const query = new URLSearchParams();
    if (options.fromSlot !== undefined) query.set("from_slot", String(options.fromSlot));
    if (options.toSlot !== undefined) query.set("to_slot", String(options.toSlot));
    if (options.limit !== undefined) query.set("limit", String(options.limit));

    const suffix = query.size > 0 ? `?${query}` : "";
    const wire = await this.get<WireHistoricalTrade[]>(
      `/v1/markets/${market}/history${suffix}`,
      signal,
    );
    return wire.map(fill);
  }

  /** OHLCV, bucketed by slot. Converting to time is the chart's job, not the indexer's. */
  async candles(
    market: string,
    options: { interval?: number; fromSlot?: number; toSlot?: number } = {},
    signal?: AbortSignal,
  ): Promise<Candle[]> {
    const query = new URLSearchParams();
    if (options.interval !== undefined) query.set("interval", String(options.interval));
    if (options.fromSlot !== undefined) query.set("from_slot", String(options.fromSlot));
    if (options.toSlot !== undefined) query.set("to_slot", String(options.toSlot));

    const suffix = query.size > 0 ? `?${query}` : "";
    const wire = await this.get<WireCandle[]>(`/v1/markets/${market}/candles${suffix}`, signal);
    return wire.map(candle);
  }
}
