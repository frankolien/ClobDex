import { useEffect } from "react";

import { Chart } from "./Chart.tsx";
import { Ladder } from "./Ladder.tsx";
import { Position } from "./Position.tsx";
import { Tape } from "./Tape.tsx";
import { bps, compact, price, shortAddress, signed } from "../lib/format.ts";
import { useApp } from "../lib/store.ts";

/** What the feed's status word should say, in the language of the thing being watched. */
const STATUS: Record<string, string> = {
  connecting: "connecting",
  live: "live",
  gapped: "reconnecting — messages were dropped",
  closed: "disconnected — showing the last known book",
};

export function Trade({ market }: { market: string | null }) {
  const markets = useApp((state) => state.markets);
  const feed = useApp((state) => state.feed);
  const window = useApp((state) => state.window);
  const watch = useApp((state) => state.watch);
  const unwatch = useApp((state) => state.unwatch);
  const loadMarkets = useApp((state) => state.loadMarkets);

  useEffect(() => {
    if (markets === null) void loadMarkets();
  }, [markets, loadMarkets]);

  useEffect(() => {
    if (!market) return;
    watch(market);
    // The subscription belongs to this view. Leaving it running after navigating away
    // would keep a socket open for a market nobody is looking at.
    return () => unwatch();
  }, [market, watch, unwatch]);

  if (!market) {
    return <div className="placeholder">Choose a market from the list.</div>;
  }

  const summary = markets?.find((entry) => entry.market === market);
  if (!summary) {
    return (
      <div className="placeholder">
        {markets === null ? "Loading…" : "The indexer is not tracking this market."}
      </div>
    );
  }

  const lots = summary.lots;

  return (
    <div className="trade">
      <div className="ticker panel">
        <div className="pair">
          <b>
            {shortAddress(summary.baseMint)}/{shortAddress(summary.quoteMint)}
          </b>
          <span className="num muted" title={market}>
            {shortAddress(market)}
          </span>
        </div>

        <Figure label="Last" value={price(lots, summary.lastPriceInTicks)} />
        <Figure label="Bid" value={price(lots, summary.bestBidInTicks)} tone="bid" />
        <Figure label="Ask" value={price(lots, summary.bestAskInTicks)} tone="ask" />
        <Figure label="Spread" value={price(lots, summary.spreadInTicks)} />
        <Figure
          label="24h change"
          value={signed(lots, window?.changeInTicks ?? null)}
          tone={changeTone(window?.changeInTicks ?? null)}
        />
        <Figure label="24h volume" value={window ? compact(window.baseLots) : "—"} />
        <Figure label="Taker fee" value={bps(summary.takerFeeBps)} />

        <span className={`pip ${feed.status}`}>{STATUS[feed.status] ?? feed.status}</span>
      </div>

      {/* The window's own honesty flag. A total assembled from a capped read is a floor,
          and a floor presented as a total is a venue under-reporting itself. */}
      {window?.truncated && (
        <p className="stale">
          More fills in this window than one query may read — volume is a floor, and the
          open is not the window's open.
        </p>
      )}

      <div className="grid">
        <Chart market={market} lots={lots} slot={feed.slot} />
        <Ladder feed={feed} lots={lots} />
        <Tape feed={feed} lots={lots} />
        <Position lots={lots} />
      </div>
    </div>
  );
}

function changeTone(change: bigint | null): "bid" | "ask" | undefined {
  if (change === null || change === 0n) return undefined;
  return change > 0n ? "bid" : "ask";
}

function Figure({ label, value, tone }: { label: string; value: string; tone?: "bid" | "ask" | undefined }) {
  return (
    <div className="figure">
      <span className="l">{label}</span>
      <span className={`v num ${tone ?? ""}`}>{value}</span>
    </div>
  );
}
