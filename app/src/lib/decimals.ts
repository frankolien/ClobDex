/**
 * Mint decimals, fetched once and remembered.
 *
 * A module-level cache rather than store state: decimals for a given mint never change, so
 * there is nothing to invalidate and nothing to keep in sync. Two components asking for the
 * same market get the same answer and one request.
 */

import { useEffect, useState } from "react";

import { config } from "./config.ts";
import { type Decimals, ASSUMED } from "./format.ts";
import { fetchDecimals } from "./mint.ts";

/** Mint address to decimals, for everything resolved so far this session. */
const known = new Map<string, number>();

/** Mints already asked about, so a failure is not retried on every render. */
const asked = new Set<string>();

/**
 * The decimals for one market, falling back to the assumed shape until the chain answers.
 *
 * Returns the fallback rather than `null` on purpose. A ladder that renders nothing while
 * a mint resolves is a ladder that flickers on every navigation, and the assumed values are
 * right for the markets this deploy is pointed at — the risk is a market with a different
 * shape briefly showing shifted numbers, which is visibly wrong rather than quietly wrong.
 */
export function useDecimals(baseMint: string | undefined, quoteMint: string | undefined): Decimals {
  const [, bump] = useState(0);

  useEffect(() => {
    if (!baseMint || !quoteMint) return;
    const wanted = [baseMint, quoteMint].filter((mint) => !asked.has(mint));
    if (wanted.length === 0) return;

    for (const mint of wanted) asked.add(mint);
    const controller = new AbortController();

    void fetchDecimals(config.rpcUrl, wanted, controller.signal)
      .then((found) => {
        for (const [mint, decimals] of found) known.set(mint, decimals);
        if (found.size > 0 && !controller.signal.aborted) bump((count) => count + 1);
      })
      .catch(() => {
        // Left unknown, so the fallback stands. Retrying on a loop against an RPC that is
        // refusing would be a lot of requests to keep displaying the same numbers.
      });

    return () => controller.abort();
  }, [baseMint, quoteMint]);

  // `??` rather than `||`, and an explicit undefined check rather than `mint && …`: a
  // decimals of 0 is legitimate and both shortcuts would replace it with the fallback.
  return {
    base: (baseMint === undefined ? undefined : known.get(baseMint)) ?? ASSUMED.base,
    quote: (quoteMint === undefined ? undefined : known.get(quoteMint)) ?? ASSUMED.quote,
  };
}

/** Whether both mints of a market have been read, for anything that wants to say so. */
export function resolved(baseMint: string, quoteMint: string): boolean {
  return known.has(baseMint) && known.has(quoteMint);
}
