import { useEffect } from "react";

import { bps, price, shortAddress, size } from "../lib/format.ts";
import { go, tradeHref } from "../lib/router.ts";
import { pollMarkets, useApp } from "../lib/store.ts";

/**
 * The markets list.
 *
 * One request serves the whole table — price, spread, depth and the lot geometry needed to
 * format any of it — because the indexer keeps all of that in memory. Rolling volume is
 * absent for the opposite reason: it costs a store query per market, so it belongs to the
 * market being watched rather than to every row.
 */
export function Markets() {
  const markets = useApp((state) => state.markets);
  const error = useApp((state) => state.error);

  // The poll is started and stopped by this component, so a second mount under strict mode
  // cannot leave an interval running with nothing reading it.
  useEffect(() => pollMarkets(), []);

  if (markets === null) {
    return <Placeholder>{error ? `Cannot reach the indexer — ${error}` : "Loading markets…"}</Placeholder>;
  }

  if (markets.length === 0) {
    return (
      <Placeholder>
        No markets are being tracked. The indexer follows whatever the program owns, so
        either none has been created or it has not caught up yet.
      </Placeholder>
    );
  }

  return (
    <div className="markets">
      {error && <p className="stale">Showing the last good response — {error}</p>}

      <table>
        <thead>
          <tr>
            <th>Market</th>
            <th className="r">Last</th>
            <th className="r">Bid</th>
            <th className="r">Ask</th>
            <th className="r">Spread</th>
            <th className="r">Depth</th>
            <th className="r">Fee</th>
            <th className="r">Seats</th>
          </tr>
        </thead>
        <tbody>
          {markets.map((market) => (
            <tr
              key={market.market}
              tabIndex={0}
              onClick={() => go(tradeHref(market.market))}
              onKeyDown={(event) => event.key === "Enter" && go(tradeHref(market.market))}
            >
              <td>
                <span className="pair">{shortAddress(market.baseMint)}/{shortAddress(market.quoteMint)}</span>
                <span className="addr num" title={market.market}>
                  {shortAddress(market.market)}
                </span>
              </td>
              <td className="r num">{price(market.lots, market.lastPriceInTicks)}</td>
              <td className="r num bid">{price(market.lots, market.bestBidInTicks)}</td>
              <td className="r num ask">{price(market.lots, market.bestAskInTicks)}</td>
              <td className="r num muted">{price(market.lots, market.spreadInTicks)}</td>
              {/* Orders resting, not value: the summary carries counts, and inventing a
                  notional from them would be a number the indexer never reported. */}
              <td className="r num muted">
                {market.bidOrders} / {market.askOrders}
              </td>
              <td className="r num muted">{bps(market.takerFeeBps)}</td>
              <td className="r num muted">{market.seats}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <p className="foot">
        Depth is resting orders per side. Prices assume {size(markets[0]!.lots, 1000n)} is
        one base unit — the app does not read mint decimals yet, so a market with a
        different shape will display shifted.
      </p>
    </div>
  );
}

function Placeholder({ children }: { children: React.ReactNode }) {
  return <div className="placeholder">{children}</div>;
}
