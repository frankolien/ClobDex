import { useState } from "react";
import { useWalletUiSigner } from "@wallet-ui/react";
import type { UiWalletAccount } from "@wallet-standard/react";

import {
  type MarketAddresses,
  TOKEN_PROGRAM_ADDRESS,
  Side,
  cancelAllOrders,
  cancelOrder,
} from "@clobdex/sdk";
import type { LotConfig } from "@clobdex/sdk";

import { useWallet } from "./Wallet.tsx";
import { config } from "../lib/config.ts";
import type { MarketSummary, OpenOrder } from "../lib/decode.ts";
import { type Decimals, price, size } from "../lib/format.ts";
import { send } from "../lib/tx.ts";
import { useApp } from "../lib/store.ts";

/**
 * The connected wallet's balances and resting orders in this market.
 *
 * Free and locked are apart because they answer different questions: a wallet that
 * deposited and then quoted still owns all of it, but only the free part can be withdrawn
 * or committed to something new. One combined number matches neither the vault nor the
 * wallet's own arithmetic.
 *
 * No seat is a different state from an empty one, and the indexer distinguishes them with
 * a 404 rather than a row of zeroes, so this does too.
 */
export function Position({
  lots,
  summary,
  decimals,
}: {
  lots: LotConfig;
  summary: MarketSummary;
  decimals: Decimals;
}) {
  const trader = useApp((state) => state.trader);
  const position = useApp((state) => state.position);
  const { account } = useWallet();

  // Cancelling is only offered when the connected wallet is the one being looked at.
  // Watching somebody else's position is a read, and a cancel button over it would build a
  // transaction their wallet has to sign and yours cannot.
  const own = account !== undefined && account.address === trader;

  if (!trader) {
    return (
      <Shell>
        <p className="empty">Connect a wallet, or watch one, to see its position here.</p>
      </Shell>
    );
  }

  if (!position) {
    return (
      <Shell>
        <p className="empty">
          This wallet holds no seat in this market. Claiming one is permissionless and costs
          227 compute units.
        </p>
      </Shell>
    );
  }

  return (
    <Shell seat={position.seat}>
      <div className="balances">
        <Balance label="Base free" value={size(lots, position.baseLotsFree, decimals)} />
        <Balance label="Base locked" value={size(lots, position.baseLotsLocked, decimals)} muted />
        <Balance label="Quote free" value={price(lots, position.quoteLotsFree, decimals)} />
        <Balance label="Quote locked" value={price(lots, position.quoteLotsLocked, decimals)} muted />
      </div>

      <div className="cols">
        <span>Open orders</span>
        <span className="r">Size</span>
        <span className="r" />
      </div>

      <ol className="orders">
        {position.orders.map((order) => (
          // Keyed by the identity the program uses, which is unique within a market: price
          // plus the side-encoded sequence number. An index key would reorder wrongly when
          // an order in the middle is filled.
          <li key={`${order.priceInTicks}-${order.orderSequenceNumber}`}>
            <span className={`num ${order.side === "bid" ? "bid" : "ask"}`}>
              {price(lots, order.priceInTicks, decimals)}
            </span>
            <span className="num r">{size(lots, order.baseLots, decimals)}</span>
            {own && account ? (
              <Cancel account={account} summary={summary} order={order} />
            ) : (
              <span />
            )}
          </li>
        ))}
      </ol>

      {position.orders.length === 0 && <p className="empty">Nothing resting.</p>}

      {own && account && position.orders.length > 1 && (
        <CancelAll account={account} summary={summary} orders={position.orders} />
      )}
    </Shell>
  );
}

function Shell({ children, seat }: { children: React.ReactNode; seat?: number }) {
  return (
    <section className="panel position">
      <div className="head">
        <span>Position</span>
        {seat !== undefined && <span className="num muted">seat {seat}</span>}
      </div>
      {children}
    </section>
  );
}

/**
 * Cancels one resting order.
 *
 * Sends `order_sequence_number`, not `sequence_number`. Bids store the complement of the
 * arrival counter — that is what makes one ascending comparison price-time priority on both
 * sides — so a client that sent the decoded one would cancel nothing, on bids only, and be
 * told nothing.
 */
function Cancel({
  account,
  summary,
  order,
}: {
  account: UiWalletAccount;
  summary: MarketSummary;
  order: OpenOrder;
}) {
  const signer = useWalletUiSigner({ account });
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  return (
    <button
      type="button"
      className="cancel"
      disabled={pending}
      title={failed ?? "Cancel this order"}
      onClick={async () => {
        setPending(true);
        setFailed(null);
        try {
          await send(config.rpcUrl, signer, [
            cancelOrder(addressesOf(summary), account.address, {
              priceInTicks: order.priceInTicks,
              orderSequenceNumber: order.orderSequenceNumber,
            }),
          ]);
        } catch (error) {
          setFailed(error instanceof Error ? error.message : String(error));
        } finally {
          setPending(false);
        }
      }}
    >
      {pending ? "…" : failed ? "!" : "✕"}
    </button>
  );
}

/**
 * Cancels everything resting, in one transaction.
 *
 * `CancelAllOrders` covers one side and takes a bound, so this is two instructions rather
 * than one — but still one transaction, which is the part that matters. Eight separate
 * transactions would be eight signatures, eight slots of exposure and eight chances to
 * land in a different order than intended.
 *
 * The bound is the number actually resting on each side rather than a large constant: the
 * program charges for what it walks, and asking it to consider orders that are not there
 * is compute spent on nothing.
 */
function CancelAll({
  account,
  summary,
  orders,
}: {
  account: UiWalletAccount;
  summary: MarketSummary;
  orders: readonly OpenOrder[];
}) {
  const signer = useWalletUiSigner({ account });
  const [pending, setPending] = useState(false);

  const bids = orders.filter((order) => order.side === "bid").length;
  const asks = orders.length - bids;

  return (
    <button
      type="button"
      className="btn cancel-all"
      disabled={pending}
      onClick={async () => {
        setPending(true);
        try {
          const addresses = addressesOf(summary);
          const instructions = [];
          if (bids > 0) {
            instructions.push(cancelAllOrders(addresses, account.address, Side.Bid, bids));
          }
          if (asks > 0) {
            instructions.push(cancelAllOrders(addresses, account.address, Side.Ask, asks));
          }
          await send(config.rpcUrl, signer, instructions);
        } catch {
          /* The panel repolls; a failure leaves the orders visibly still there. */
        } finally {
          setPending(false);
        }
      }}
    >
      {pending ? "Cancelling…" : `Cancel all ${orders.length}`}
    </button>
  );
}

function addressesOf(summary: MarketSummary): MarketAddresses {
  return {
    programAddress: config.programAddress,
    market: summary.market,
    baseVault: summary.baseVault,
    quoteVault: summary.quoteVault,
    vaultSigner: summary.vaultSigner,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  };
}

function Balance({ label, value, muted }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="balance">
      <span className="l">{label}</span>
      <span className={`v num ${muted ? "muted" : ""}`}>{value}</span>
    </div>
  );
}
