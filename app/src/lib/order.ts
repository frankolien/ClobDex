/**
 * Whether an order can be submitted, and why not.
 *
 * Pure, and in `lib` rather than beside the form, for a reason the test runner made
 * concrete: Node runs TypeScript by stripping types, and JSX is not a type — so anything
 * in a `.tsx` file cannot be tested without a build step. Logic worth testing therefore
 * does not live in a component, which is where it wanted to be written.
 *
 * Every check here is one the program would make anyway. Making them again on the client
 * is not duplication of the rules, it is refusing to spend a wallet prompt and a fee to
 * learn something already on screen.
 */

import { Side, baseLotsFor, quoteLotsFor } from "@clobdex/sdk";
import type { LotConfig } from "@clobdex/sdk";

import type { MarketSummary, Position } from "./decode.ts";

/** What can be submitted. Each maps to one packet the program already understands. */
export type Kind = "postOnly" | "limit" | "fok";

export interface Draft {
  readonly side: Side;
  readonly kind: Kind;
  readonly lots: LotConfig;
  readonly priceInTicks: bigint | null;
  readonly baseLots: bigint | null;
  readonly touch: Pick<MarketSummary, "bestBidInTicks" | "bestAskInTicks">;
  readonly position: Position | null;
}

/** Why the order cannot be sent, or `null` if it can. */
export function validate(draft: Draft): string | null {
  const { side, kind, lots, priceInTicks, baseLots, touch, position } = draft;

  if (priceInTicks === null || priceInTicks === 0n) return "Enter a price";
  if (baseLots === null || baseLots === 0n) return "Enter a size";
  if (!position) return "No seat in this market";

  // The program rejects a post-only order that would cross, so letting one through costs a
  // prompt and a fee to learn what the touch already said. Equal prices cross — a bid at
  // the best ask lifts it — which is why these are `>=` and `<=`.
  if (kind === "postOnly") {
    if (side === Side.Bid && touch.bestAskInTicks !== null && priceInTicks >= touch.bestAskInTicks) {
      return "Would cross the ask";
    }
    if (side === Side.Ask && touch.bestBidInTicks !== null && priceInTicks <= touch.bestBidInTicks) {
      return "Would cross the bid";
    }
  }

  // Free balance only. Locked funds are already behind a resting order, and counting them
  // would offer a size the program refuses.
  //
  // The cost comes from the SDK's `quoteLotsFor` rather than being multiplied out here:
  // that is the function the program's exactness invariant is stated in terms of, and a
  // second version of the conversion on a form is a number that disagrees with what
  // actually settles.
  if (side === Side.Bid) {
    if (position.quoteLotsFree < quoteLotsFor(lots, priceInTicks, baseLots)) {
      return "Not enough free quote";
    }
  } else if (position.baseLotsFree < baseLots) {
    return "Not enough free base";
  }

  return null;
}

/**
 * The largest size this seat can afford at a price.
 *
 * Rounds down, by way of the SDK: an affordable size that rounds up is an order the
 * program declines after a signing prompt.
 */
export function maxSize(
  side: Side,
  lots: LotConfig,
  priceInTicks: bigint,
  position: Position,
): bigint {
  return side === Side.Bid
    ? baseLotsFor(lots, priceInTicks, position.quoteLotsFree)
    : position.baseLotsFree;
}
