import { test } from "node:test";
import assert from "node:assert/strict";

import { Side } from "@clobdex/sdk";
import type { LotConfig } from "@clobdex/sdk";

import { maxSize, validate } from "../src/lib/order.ts";
import type { Position } from "../src/lib/decode.ts";

/** The SOL/USDC shape the CLI creates. One base lot at one tick is one quote lot. */
const lots: LotConfig = {
  baseLotsPerBaseUnit: 1_000n,
  tickSizeInQuoteLotsPerBaseUnit: 1_000n,
  baseAtomsPerBaseLot: 1_000_000n,
  quoteAtomsPerQuoteLot: 1n,
};

const touch = { bestBidInTicks: 98n, bestAskInTicks: 102n };

function seat(overrides: Partial<Position> = {}): Position {
  return {
    market: "MKT",
    trader: "WALLET",
    seat: 1,
    slot: 1,
    finalizedThrough: 0,
    baseLotsFree: 5_000n,
    baseLotsLocked: 0n,
    quoteLotsFree: 9_000_000n,
    quoteLotsLocked: 0n,
    orders: [],
    ...overrides,
  };
}

const base = {
  side: Side.Bid,
  kind: "postOnly" as const,
  lots,
  priceInTicks: 95n,
  baseLots: 10n,
  touch,
  position: seat(),
};

test("a well-formed post-only order passes", () => {
  assert.equal(validate(base), null);
});

test("an incomplete order says which field is missing", () => {
  assert.equal(validate({ ...base, priceInTicks: null }), "Enter a price");
  assert.equal(validate({ ...base, priceInTicks: 0n }), "Enter a price");
  assert.equal(validate({ ...base, baseLots: null }), "Enter a size");
  assert.equal(validate({ ...base, baseLots: 0n }), "Enter a size");
});

test("no seat is caught before a signing prompt", () => {
  assert.equal(validate({ ...base, position: null }), "No seat in this market");
});

test("a post-only order that would cross is refused on the client", () => {
  // The program rejects this, so letting it through costs a wallet prompt and a fee to
  // learn something the touch already said.
  assert.equal(validate({ ...base, priceInTicks: 102n }), "Would cross the ask");
  assert.equal(validate({ ...base, priceInTicks: 150n }), "Would cross the ask");
  assert.equal(
    validate({ ...base, side: Side.Ask, priceInTicks: 98n, position: seat() }),
    "Would cross the bid",
  );
});

test("at the touch but not through it is still a cross", () => {
  // Equal prices cross: a bid at the best ask lifts it. The boundary is the case worth
  // pinning, because an off-by-one here rejects valid quotes or lets crossing ones past.
  assert.equal(validate({ ...base, priceInTicks: 101n }), null, "one tick inside is fine");
  assert.equal(validate({ ...base, priceInTicks: 102n }), "Would cross the ask");
});

test("a limit order is allowed to cross — that is what it is for", () => {
  assert.equal(validate({ ...base, kind: "limit", priceInTicks: 150n }), null);
  assert.equal(validate({ ...base, kind: "fok", priceInTicks: 150n }), null);
});

test("a one-sided book does not block the side that has no quote", () => {
  // A market with no asks cannot have a bid that crosses one.
  assert.equal(
    validate({ ...base, priceInTicks: 5_000n, touch: { bestBidInTicks: 98n, bestAskInTicks: null } }),
    null,
  );
});

test("affordability is checked against free balance, not total", () => {
  // Locked funds are already behind a resting order. Counting them would offer a size the
  // program refuses, after a prompt.
  const rich = seat({ quoteLotsFree: 950n, quoteLotsLocked: 9_000_000n });
  // 10 lots at 95 ticks costs 950 quote lots at this geometry — exactly affordable.
  assert.equal(validate({ ...base, position: rich }), null);
  assert.equal(
    validate({ ...base, baseLots: 11n, position: rich }),
    "Not enough free quote",
  );
});

test("selling checks the base side", () => {
  // Nine free and five thousand locked behind resting asks: sellable size is nine, not
  // 5,009. This is the whole reason the two are reported separately.
  const short = seat({ baseLotsFree: 9n, baseLotsLocked: 5_000n });
  assert.equal(
    validate({ ...base, side: Side.Ask, priceInTicks: 150n, baseLots: 9n, position: short }),
    null,
  );
  assert.equal(
    validate({ ...base, side: Side.Ask, priceInTicks: 150n, baseLots: 10n, position: short }),
    "Not enough free base",
  );
});

test("max size spends free balance and rounds down", () => {
  // A size that rounded up would be an order the program declines, after a prompt.
  const position = seat({ quoteLotsFree: 999n, baseLotsFree: 42n });
  assert.equal(maxSize(Side.Bid, lots, 100n, position), 9n, "999 buys nine lots at 100, not ten");
  assert.equal(maxSize(Side.Ask, lots, 100n, position), 42n, "selling spends base directly");
});
