import type { LotConfig } from "@clobdex/sdk";

import { price, size } from "../lib/format.ts";
import { useApp } from "../lib/store.ts";

/**
 * The connected wallet's balances and resting orders in this market.
 *
 * Free and locked are shown apart because they answer different questions: a wallet that
 * deposited and then quoted still owns all of it, but only the free part can be withdrawn
 * or committed to something new. One combined number matches neither the vault nor the
 * wallet's own arithmetic.
 *
 * No seat is a different state from an empty one, and the indexer distinguishes them with
 * a 404 rather than a row of zeroes, so this does too.
 */
export function Position({ lots }: { lots: LotConfig }) {
  const trader = useApp((state) => state.trader);
  const position = useApp((state) => state.position);

  if (!trader) {
    return (
      <section className="panel position">
        <div className="head">
          <span>Position</span>
        </div>
        <p className="empty">Watch a wallet to see its balances and open orders here.</p>
      </section>
    );
  }

  if (!position) {
    return (
      <section className="panel position">
        <div className="head">
          <span>Position</span>
        </div>
        <p className="empty">
          This wallet holds no seat in this market. Claiming one is permissionless and
          costs 227 compute units.
        </p>
      </section>
    );
  }

  return (
    <section className="panel position">
      <div className="head">
        <span>Position</span>
        <span className="num muted">seat {position.seat}</span>
      </div>

      <div className="balances">
        <Balance label="Base free" value={size(lots, position.baseLotsFree)} />
        <Balance label="Base locked" value={size(lots, position.baseLotsLocked)} muted />
        <Balance label="Quote free" value={price(lots, position.quoteLotsFree)} />
        <Balance label="Quote locked" value={price(lots, position.quoteLotsLocked)} muted />
      </div>

      <div className="cols">
        <span>Open orders</span>
        <span className="r">Size</span>
      </div>

      <ol className="orders">
        {position.orders.map((order) => (
          // Keyed by the identity the program uses, which is unique per market: price plus
          // the side-encoded sequence number. An index key would reorder wrongly when one
          // order in the middle is filled.
          <li key={`${order.priceInTicks}-${order.orderSequenceNumber}`}>
            <span className={`num ${order.side === "bid" ? "bid" : "ask"}`}>
              {price(lots, order.priceInTicks)}
            </span>
            <span className="num r">{size(lots, order.baseLots)}</span>
          </li>
        ))}
      </ol>

      {position.orders.length === 0 && <p className="empty">Nothing resting.</p>}

      <footer>
        Read-only. Placing and cancelling needs a connected wallet, which this build cannot
        do yet.
      </footer>
    </section>
  );
}

function Balance({ label, value, muted }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="balance">
      <span className="l">{label}</span>
      <span className={`v num ${muted ? "muted" : ""}`}>{value}</span>
    </div>
  );
}
