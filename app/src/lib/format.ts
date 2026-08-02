/**
 * Turning exact quantities into things a person reads.
 *
 * The only place in the app where precision is deliberately lost, and it happens at the
 * last moment before a string reaches the screen. Everything upstream is `bigint`, and
 * everything these return is a string — so a formatted value cannot be fed back into
 * arithmetic that expected exactness.
 *
 * The lot maths itself comes from the SDK rather than being repeated here. It is
 * conformance-tested against the Rust builders, and a second implementation of a tick
 * conversion is exactly the drift this repository keeps refusing to introduce.
 */

import { type LotConfig, formatPrice, formatSize } from "@clobdex/sdk";

/**
 * Decimals used until a mint has been read.
 *
 * Nine and six are the SOL/USDC shape the CLI creates. They are a starting point for the
 * first paint only — `useDecimals` replaces them as soon as the mints resolve — and being
 * wrong about them shifts a displayed number by orders of magnitude, so nothing should
 * rely on them past that first frame.
 */
export const ASSUMED_BASE_DECIMALS = 9;
export const ASSUMED_QUOTE_DECIMALS = 6;

/** How many atoms make one whole token, for each side of a market. */
export interface Decimals {
  readonly base: number;
  readonly quote: number;
}

/** What to use before the chain has answered. */
export const ASSUMED: Decimals = { base: ASSUMED_BASE_DECIMALS, quote: ASSUMED_QUOTE_DECIMALS };

/** A price in ticks, as a human price. */
export function price(lots: LotConfig, ticks: bigint | null, decimals: Decimals = ASSUMED): string {
  return ticks === null ? "—" : formatPrice(lots, ticks, decimals.quote);
}

/** A size in base lots, as a human size. */
export function size(lots: LotConfig, baseLots: bigint, decimals: Decimals = ASSUMED): string {
  return formatSize(lots, baseLots, decimals.base);
}

/**
 * A large count, abbreviated.
 *
 * For volume figures where the magnitude is the point and the last three digits are not.
 * Never used for a price or an order size, where a rounded number is a wrong number.
 */
export function compact(value: bigint): string {
  const units: [bigint, string][] = [
    [1_000_000_000n, "B"],
    [1_000_000n, "M"],
    [1_000n, "K"],
  ];
  for (const [scale, suffix] of units) {
    if (value >= scale) {
      // One decimal place, computed in bigint so the division never goes through a double.
      const tenths = (value * 10n) / scale;
      return `${tenths / 10n}.${tenths % 10n}${suffix}`;
    }
  }
  return value.toString();
}

/** A basis-point fee, as a percentage. */
export function bps(value: number): string {
  return `${(value / 100).toFixed(2)}%`;
}

/** An address, shortened for a table. The full value belongs in a title attribute. */
export function shortAddress(address: string): string {
  return address.length <= 12 ? address : `${address.slice(0, 4)}…${address.slice(-4)}`;
}

/**
 * A signed change, with its sign always shown.
 *
 * A gain rendered without a `+` reads as a number rather than as a direction, and the two
 * are easy to confuse at a glance in a column of them.
 */
export function signed(
  lots: LotConfig,
  ticks: bigint | null,
  decimals: Decimals = ASSUMED,
): string {
  if (ticks === null) return "—";
  const magnitude = formatPrice(lots, ticks < 0n ? -ticks : ticks, decimals.quote);
  return `${ticks < 0n ? "−" : "+"}${magnitude}`;
}
