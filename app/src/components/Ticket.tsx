import { useState } from "react";
import { useWalletUiSigner } from "@wallet-ui/react";
import type { UiWalletAccount } from "@wallet-standard/react";

import {
  type MarketAddresses,
  PostOnlyRejection,
  Side,
  TOKEN_PROGRAM_ADDRESS,
  fillOrKill,
  limit,
  placeOrder,
  postOnly,
  quoteLotsFor,
} from "@clobdex/sdk";
import type { LotConfig } from "@clobdex/sdk";

import { useWallet } from "./Wallet.tsx";
import { config } from "../lib/config.ts";
import type { MarketSummary } from "../lib/decode.ts";
import { type Decimals, price, size } from "../lib/format.ts";
import { type Kind, maxSize, validate } from "../lib/order.ts";
import { send } from "../lib/tx.ts";
import { useApp } from "../lib/store.ts";

/** The labels, in the order they are offered. The ids come from `lib/order.ts`. */
const KINDS = [
  { id: "postOnly", label: "Post only", hint: "Rejects rather than crossing. Never pays a taker fee." },
  { id: "limit", label: "Limit", hint: "Crosses what it can and rests the remainder." },
  { id: "fok", label: "Fill or kill", hint: "Fills entirely at this price or does nothing." },
] as const satisfies readonly { id: Kind; label: string; hint: string }[];

/**
 * Order entry.
 *
 * Split in two because `useWalletUiSigner` needs an account and hooks cannot be called
 * conditionally. The outer component decides whether there is a wallet; the inner one only
 * ever renders when there is, so it can call the hook unconditionally at its top. The
 * alternative — stashing the signer in a module variable — works until two of these are on
 * screen at once and then silently signs with the wrong one.
 */
export function Ticket({
  summary,
  lots,
  decimals,
}: {
  summary: MarketSummary;
  lots: LotConfig;
  decimals: Decimals;
}) {
  const { account } = useWallet();

  if (!account) {
    return (
      <section className="panel ticket">
        <div className="head">
          <span>Order</span>
        </div>
        <p className="empty">Connect a wallet to place an order.</p>
      </section>
    );
  }

  return <Form account={account} summary={summary} lots={lots} decimals={decimals} />;
}

function Form({
  account,
  summary,
  lots,
  decimals,
}: {
  account: UiWalletAccount;
  summary: MarketSummary;
  lots: LotConfig;
  decimals: Decimals;
}) {
  const signer = useWalletUiSigner({ account });
  const position = useApp((state) => state.position);

  const [side, setSide] = useState<Side>(Side.Bid);
  const [kind, setKind] = useState<Kind>("postOnly");
  const [ticks, setTicks] = useState("");
  const [lotsText, setLotsText] = useState("");
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const parsedTicks = parseQuantity(ticks);
  const parsedLots = parseQuantity(lotsText);

  // Quoted with the SDK's own lot maths, not recomputed here. It is the function the
  // program's exactness invariant is stated in terms of, and a second version of it on a
  // form would be a number that disagrees with what settles.
  const cost =
    parsedTicks !== null && parsedLots !== null && parsedLots > 0n
      ? quoteLotsFor(lots, parsedTicks, parsedLots)
      : null;

  const problem = validate({
    side,
    kind,
    lots,
    priceInTicks: parsedTicks,
    baseLots: parsedLots,
    touch: summary,
    position,
  });

  const addresses: MarketAddresses = {
    programAddress: config.programAddress,
    market: summary.market,
    baseVault: summary.baseVault,
    quoteVault: summary.quoteVault,
    vaultSigner: summary.vaultSigner,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  };

  async function submit() {
    if (parsedTicks === null || parsedLots === null) return;
    setPending(true);
    setResult(null);
    try {
      const packet =
        kind === "postOnly"
          ? postOnly(side, parsedTicks, parsedLots, PostOnlyRejection.Reject)
          : kind === "fok"
            ? fillOrKill(side, parsedTicks, parsedLots)
            : limit(side, parsedTicks, parsedLots);

      const signature = await send(config.rpcUrl, signer, [
        placeOrder(addresses, account.address, packet),
      ]);
      setResult({ ok: true, text: signature });
    } catch (error) {
      // Shown verbatim. A wallet rejection, a failed simulation and a stale blockhash need
      // different responses, and collapsing them into "failed" removes the one clue that
      // tells them apart.
      setResult({ ok: false, text: error instanceof Error ? error.message : String(error) });
    } finally {
      setPending(false);
    }
  }

  return (
    <section className="panel ticket">
      <div className="head">
        <span>Order</span>
        <span className="muted num">{summary.takerFeeBps} bps taker</span>
      </div>

      <div className="sides">
        <button type="button" className={side === Side.Bid ? "on bid" : ""} onClick={() => setSide(Side.Bid)}>
          Buy
        </button>
        <button type="button" className={side === Side.Ask ? "on ask" : ""} onClick={() => setSide(Side.Ask)}>
          Sell
        </button>
      </div>

      <div className="kinds">
        {KINDS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            className={kind === entry.id ? "on" : ""}
            onClick={() => setKind(entry.id)}
            title={entry.hint}
          >
            {entry.label}
          </button>
        ))}
      </div>

      <label>
        <span>Price, in ticks</span>
        <input
          className="num"
          inputMode="numeric"
          value={ticks}
          placeholder={summary.midPriceInTicks?.toString() ?? "0"}
          onChange={(event) => setTicks(event.target.value)}
        />
        <em>{parsedTicks === null ? "—" : price(lots, parsedTicks, decimals)}</em>
      </label>

      <label>
        <span>Size, in base lots</span>
        <input
          className="num"
          inputMode="numeric"
          value={lotsText}
          placeholder="0"
          onChange={(event) => setLotsText(event.target.value)}
        />
        <em>{parsedLots === null ? "—" : size(lots, parsedLots, decimals)}</em>
      </label>

      {/* Affordability from free balance only. Locked funds are already behind a resting
          order, and counting them would offer a size the program refuses. */}
      {position && parsedTicks !== null && parsedTicks > 0n && (
        <button
          type="button"
          className="max"
          onClick={() =>
            setLotsText(String(maxSize(side, lots, parsedTicks, position)))
          }
        >
          Max from free balance
        </button>
      )}

      <dl className="summary">
        <div>
          <dt>Cost</dt>
          <dd className="num">{cost === null ? "—" : price(lots, cost, decimals)}</dd>
        </div>
        <div>
          <dt>Fee</dt>
          <dd className="num">{kind === "postOnly" ? "none — maker" : `${summary.takerFeeBps} bps`}</dd>
        </div>
      </dl>

      <button
        type="button"
        className="btn btn-solid submit"
        disabled={pending || problem !== null}
        onClick={() => void submit()}
      >
        {pending ? "Confirm in your wallet…" : (problem ?? `${side === Side.Bid ? "Buy" : "Sell"}`)}
      </button>

      {result && <Outcome result={result} />}
    </section>
  );
}

function Outcome({ result }: { result: { ok: boolean; text: string } }) {
  if (!result.ok) return <p className="bad">{result.text}</p>;
  return (
    <p className="ok">
      <a
        href={`https://explorer.solana.com/tx/${result.text}?cluster=${config.cluster}`}
        target="_blank"
        rel="noopener noreferrer"
      >
        Sent — view transaction
      </a>
    </p>
  );
}

/** Digits only, as a bigint. Empty and malformed both mean "not yet a number". */
function parseQuantity(text: string): bigint | null {
  const trimmed = text.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  return BigInt(trimmed);
}
