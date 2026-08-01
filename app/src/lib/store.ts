/**
 * The seam between the tested data layer and React.
 *
 * Everything hard already happened: the reducer in {@link ./feed.ts} is pure and covered,
 * the decoders in {@link ./decode.ts} are strict, the transport in {@link ./socket.ts}
 * knows when to reconnect. This file only holds the result and tells React it changed, so
 * it is deliberately thin — a store that starts making decisions is a store whose logic
 * has escaped its tests.
 */

import { create } from "zustand";

import { Indexer } from "./api.ts";
import type { MarketSummary, Position, Window } from "./decode.ts";
import type { Book } from "./api.ts";
import { type FeedState, initial } from "./feed.ts";
import { Feed } from "./socket.ts";
import { config } from "./config.ts";

/** One client for the whole app. Nothing about it is per-component. */
export const indexer = new Indexer(config.indexerUrl);

/** How often the markets list is refreshed. It is served from memory, so this is cheap. */
const MARKETS_POLL_MS = 5_000;

/** How often a trader's balances and open orders are refreshed while a market is open. */
const POSITION_POLL_MS = 3_000;

interface AppState {
  /** Every tracked market. `null` until the first response. */
  markets: MarketSummary[] | null;
  /** What went wrong reaching the indexer, if anything. */
  error: string | null;

  /** The market being traded, if any. */
  market: string | null;
  /** The live feed for that market. */
  feed: FeedState;
  /** Rolling stats for that market. */
  window: Window | null;
  /** The connected wallet's position in that market. `null` means no seat. */
  position: Position | null;

  /** The wallet address the app is showing, if one has been given. */
  trader: string | null;

  loadMarkets: () => Promise<void>;
  watch: (market: string) => void;
  unwatch: () => void;
  setTrader: (trader: string | null) => void;
}

let feed: Feed | null = null;
let marketsTimer: ReturnType<typeof setInterval> | null = null;
let positionTimer: ReturnType<typeof setInterval> | null = null;

/** Turns whatever was thrown into something a person can read. */
function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const useApp = create<AppState>((set, get) => ({
  markets: null,
  error: null,
  market: null,
  feed: initial,
  window: null,
  position: null,
  trader: null,

  async loadMarkets() {
    try {
      set({ markets: await indexer.markets(), error: null });
    } catch (error) {
      // The list is left as it was rather than emptied. A transient failure that wipes the
      // screen is worse than one that leaves stale rows next to an error.
      set({ error: reason(error) });
    }
  },

  watch(market: string) {
    if (get().market === market) return;
    get().unwatch();

    set({ market, feed: initial, window: null, position: null });

    feed = new Feed({
      socketUrl: config.indexerSocketUrl,
      market,
      onState: (state) => set({ feed: state }),
    });
    feed.start();

    // The window is a store query, so it is polled rather than streamed. Slower than the
    // book on purpose: a 24-hour total does not move meaningfully between slots.
    const refreshWindow = async () => {
      try {
        set({ window: await indexer.window(market) });
      } catch {
        // Leave the previous figures. A missing volume is better than a zero one, which
        // would read as a market that stopped trading.
      }
    };
    void refreshWindow();

    const refreshPosition = async () => {
      const trader = get().trader;
      if (!trader) return set({ position: null });
      try {
        set({ position: await indexer.position(market, trader) });
      } catch {
        /* Same reasoning: keep what was last true. */
      }
    };
    void refreshPosition();

    marketsTimer = setInterval(refreshWindow, MARKETS_POLL_MS * 12);
    positionTimer = setInterval(refreshPosition, POSITION_POLL_MS);
  },

  unwatch() {
    feed?.stop();
    feed = null;
    if (marketsTimer) clearInterval(marketsTimer);
    if (positionTimer) clearInterval(positionTimer);
    marketsTimer = positionTimer = null;
    set({ market: null, feed: initial, window: null, position: null });
  },

  setTrader(trader: string | null) {
    set({ trader, position: null });
  },
}));

/**
 * Starts the markets poll and returns a function that stops it.
 *
 * Kept out of the store so a component's lifecycle owns it: mounting twice under React's
 * strict mode must not leave two intervals running, and the only way to guarantee that is
 * for the thing that started it to be the thing that stops it.
 */
export function pollMarkets(): () => void {
  void useApp.getState().loadMarkets();
  const timer = setInterval(() => void useApp.getState().loadMarkets(), MARKETS_POLL_MS);
  return () => clearInterval(timer);
}

export type { Book, MarketSummary, Position, Window };
