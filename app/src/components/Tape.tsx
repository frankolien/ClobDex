import { useMemo } from "react";

import type { LotConfig } from "@clobdex/sdk";

import { type FeedState, isFinal } from "../lib/feed.ts";
import { anchorAt, ago } from "../lib/time.ts";
import { type Decimals, price, size } from "../lib/format.ts";

/**
 * The trade tape.
 *
 * Two things here that most tapes get wrong and this feed makes possible to get right.
 *
 * A fill that is not yet rooted is marked. The indexer runs at `confirmed`, which sees a
 * trade about a second before finality does and accepts that its slot can still be
 * abandoned — so provisional prints are shown as provisional rather than as fact.
 *
 * Finality is re-derived from the current watermark rather than read off each row. The
 * server stamps it when it sends, so a fill that has since rooted still says otherwise on
 * the object it arrived in.
 */
export function Tape({
  feed,
  lots,
  decimals,
}: {
  feed: FeedState;
  lots: LotConfig;
  decimals: Decimals;
}) {
  // The clock is pinned once per slot advance rather than per render, so rows do not
  // recompute their age sixty times a second for no visible change.
  const anchor = useMemo(() => anchorAt(feed.slot, Date.now()), [feed.slot]);
  const now = Date.now();

  return (
    <section className="panel tape">
      <div className="head">
        <span>Fills</span>
        {feed.retracted > 0 && (
          <span className="retracted" title="Trades withdrawn because their slot was abandoned">
            {feed.retracted} retracted
          </span>
        )}
      </div>

      <div className="cols">
        <span>Price</span>
        <span className="r">Size</span>
        <span className="r">Age</span>
      </div>

      <ol>
        {feed.tape.map((fill, index) => {
          const rooted = isFinal(feed, fill);
          return (
            <li key={`${fill.slot}-${index}`} className={rooted ? "" : "pending"}>
              <span className={`num ${fill.takerSide === "bid" ? "bid" : "ask"}`}>
                {price(lots, fill.priceInTicks, decimals)}
              </span>
              <span className="num r">{size(lots, fill.baseLots, decimals)}</span>
              <span className="num r muted" title={`slot ${fill.slot}`}>
                {rooted ? ago(anchor, fill.slot, now) : "pending"}
              </span>
            </li>
          );
        })}
      </ol>

      {feed.tape.length === 0 && (
        <p className="empty">
          {feed.status === "connecting" ? "Connecting…" : "Nothing has traded since this page opened."}
        </p>
      )}

      <footer>
        Ages are estimated from slot numbers at 400ms each. The chain records slots, not
        clocks.
      </footer>
    </section>
  );
}
