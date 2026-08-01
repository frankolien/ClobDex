import type { LotConfig } from "@clobdex/sdk";

import type { Level } from "../lib/decode.ts";
import { price, size } from "../lib/format.ts";
import { type FeedState, bestAsk, bestBid, mid, spread } from "../lib/feed.ts";

/**
 * The order book.
 *
 * Asks descend to the spread and bids run down from it, which is the arrangement every
 * trader already reads — the best of both sides meeting in the middle, so the spread is
 * where your eye lands rather than something to hunt for.
 *
 * Depth bars are scaled to the largest level *shown*, not to the largest in the book. A
 * bar scaled to something off-screen is a bar that conveys nothing.
 */
export function Ladder({ feed, lots, rows = 12 }: { feed: FeedState; lots: LotConfig; rows?: number }) {
  const asks = feed.asks.slice(0, rows);
  const bids = feed.bids.slice(0, rows);

  const widest = [...asks, ...bids].reduce((max, level) => (level.baseLots > max ? level.baseLots : max), 1n);
  const midpoint = mid(feed);
  const gap = spread(feed);

  return (
    <section className="panel ladder">
      <div className="head">
        <span>Order book</span>
        <span className="num">{feed.slot > 0 ? `slot ${feed.slot.toLocaleString("en-US")}` : "—"}</span>
      </div>

      <div className="cols">
        <span>Price</span>
        <span className="r">Size</span>
        <span className="r">Total</span>
      </div>

      {/* Reversed so the best ask sits against the spread. The feed sends both sides best
          first; only the display order differs. */}
      <ol className="side">
        {[...asks].reverse().map((level, index) => (
          <Row key={`a${index}`} level={level} lots={lots} widest={widest} side="ask" rest={asks.slice(0, asks.length - index)} />
        ))}
      </ol>

      <div className="spread">
        <span className="mark num">{price(lots, midpoint)}</span>
        <span className="gap num muted">
          {gap === null ? "no spread" : `${price(lots, gap)} spread`}
        </span>
      </div>

      <ol className="side">
        {bids.map((level, index) => (
          <Row key={`b${index}`} level={level} lots={lots} widest={widest} side="bid" rest={bids.slice(0, index + 1)} />
        ))}
      </ol>

      {asks.length === 0 && bids.length === 0 && (
        <p className="empty">
          {feed.status === "connecting"
            ? "Waiting for a snapshot…"
            : "No resting liquidity. An empty book is a real state, not an error."}
        </p>
      )}

      <footer className="touch">
        <span>
          Bid <b className="num bid">{price(lots, bestBid(feed))}</b>
        </span>
        <span>
          Ask <b className="num ask">{price(lots, bestAsk(feed))}</b>
        </span>
      </footer>
    </section>
  );
}

function Row({
  level,
  lots,
  widest,
  side,
  rest,
}: {
  level: Level;
  lots: LotConfig;
  widest: bigint;
  side: "bid" | "ask";
  rest: Level[];
}) {
  // Cumulative depth to this level, summed in bigint. A running total in doubles loses
  // lots at exactly the depth where the total starts to matter.
  const total = rest.reduce((sum, entry) => sum + entry.baseLots, 0n);
  // Percentages are the one place a double is fine: it is a bar width, not a quantity.
  const fill = Number((level.baseLots * 10_000n) / widest) / 100;

  return (
    <li className={side}>
      <span className="bar" style={{ width: `${fill}%` }} />
      <span className={`p num ${side}`}>{price(lots, level.priceInTicks)}</span>
      <span className="s num r">{size(lots, level.baseLots)}</span>
      <span className="t num r muted">{size(lots, total)}</span>
    </li>
  );
}
