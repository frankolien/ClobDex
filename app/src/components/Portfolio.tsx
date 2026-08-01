import { useEffect, useState } from "react";

import { price, shortAddress, size } from "../lib/format.ts";
import { go, tradeHref } from "../lib/router.ts";
import { type Position, indexer, pollMarkets, useApp } from "../lib/store.ts";

/**
 * One wallet's position across every market.
 *
 * The indexer answers per market, so this asks each one. That is fine at the scale a
 * devnet deployment runs at and would not be at a hundred markets — the honest fix then is
 * an endpoint that answers for a trader across markets, not a hundred requests from a
 * browser. Worth writing down here rather than discovering it later.
 */
export function Portfolio() {
  const markets = useApp((state) => state.markets);
  const trader = useApp((state) => state.trader);
  const [positions, setPositions] = useState<Position[] | null>(null);

  useEffect(() => pollMarkets(), []);

  useEffect(() => {
    if (!trader || markets === null) return setPositions(null);
    const controller = new AbortController();

    void (async () => {
      const found = await Promise.all(
        markets.map((market) =>
          indexer.position(market.market, trader, controller.signal).catch(() => null),
        ),
      );
      if (!controller.signal.aborted) setPositions(found.filter((entry) => entry !== null));
    })();

    return () => controller.abort();
  }, [trader, markets]);

  if (!trader) {
    return <div className="placeholder">Watch a wallet to see its positions.</div>;
  }

  if (positions === null) {
    return <div className="placeholder">Loading positions…</div>;
  }

  if (positions.length === 0) {
    return (
      <div className="placeholder">
        <span className="num">{shortAddress(trader)}</span> holds no seat in any tracked
        market. A seat is claimed per market and costs 227 compute units.
      </div>
    );
  }

  return (
    <div className="portfolio">
      {positions.map((position) => {
        const summary = markets?.find((entry) => entry.market === position.market);
        if (!summary) return null;
        const lots = summary.lots;

        return (
          <section className="panel" key={position.market}>
            <div className="head">
              <a href={tradeHref(position.market)} onClick={(event) => {
                if (event.metaKey || event.ctrlKey || event.shiftKey) return;
                event.preventDefault();
                go(tradeHref(position.market));
              }}>
                {shortAddress(summary.baseMint)}/{shortAddress(summary.quoteMint)}
              </a>
              <span className="num muted">seat {position.seat}</span>
            </div>

            <div className="balances">
              <Cell label="Base free" value={size(lots, position.baseLotsFree)} />
              <Cell label="Base locked" value={size(lots, position.baseLotsLocked)} muted />
              <Cell label="Quote free" value={price(lots, position.quoteLotsFree)} />
              <Cell label="Quote locked" value={price(lots, position.quoteLotsLocked)} muted />
              <Cell label="Open orders" value={String(position.orders.length)} />
            </div>
          </section>
        );
      })}
    </div>
  );
}

function Cell({ label, value, muted }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="balance">
      <span className="l">{label}</span>
      <span className={`v num ${muted ? "muted" : ""}`}>{value}</span>
    </div>
  );
}
