import { test } from "node:test";
import assert from "node:assert/strict";

import vectors from "../../../spec/wire-vectors.json" with { type: "json" };
import { DECODED_LENGTH, MarketDecodeError, decodeMarket } from "../src/market.ts";
import * as lots from "../src/lots.ts";
import { SizeClass } from "../src/types.ts";

/** The account bytes Rust wrote, as bytes. */
function account(): Uint8Array {
  const hex = vectors.marketAccount.bytes;
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

const expected = vectors.marketAccount.decoded;

test("decodes every field Rust said it wrote", () => {
  // Each numeric field in the fixture has a distinct value, so a decoder reading the
  // wrong offset gets a wrong answer rather than a plausible zero.
  const market = decodeMarket(account());

  assert.equal(market.sizeClass, SizeClass.Small);
  assert.equal(market.vaultSignerBump, expected.vaultSignerBump);
  assert.equal(market.baseMint, expected.baseMint);
  assert.equal(market.quoteMint, expected.quoteMint);
  assert.equal(market.baseVault, expected.baseVault);
  assert.equal(market.quoteVault, expected.quoteVault);
  assert.equal(market.authority, expected.authority);
  assert.equal(market.feeRecipient, expected.feeRecipient);

  assert.equal(market.takerFeeBps, BigInt(expected.takerFeeBps));
  assert.equal(market.baseLotsDeposited, BigInt(expected.baseLotsDeposited));
  assert.equal(market.quoteLotsDeposited, BigInt(expected.quoteLotsDeposited));
  assert.equal(market.collectedQuoteLotFees, BigInt(expected.collectedQuoteLotFees));
  assert.equal(market.unclaimedQuoteLotFees, BigInt(expected.unclaimedQuoteLotFees));

  assert.equal(market.lotConfig.baseLotsPerBaseUnit, BigInt(expected.lotConfig.baseLotsPerBaseUnit));
  assert.equal(
    market.lotConfig.tickSizeInQuoteLotsPerBaseUnit,
    BigInt(expected.lotConfig.tickSizeInQuoteLotsPerBaseUnit),
  );
  assert.equal(market.lotConfig.baseAtomsPerBaseLot, BigInt(expected.lotConfig.baseAtomsPerBaseLot));
  assert.equal(
    market.lotConfig.quoteAtomsPerQuoteLot,
    BigInt(expected.lotConfig.quoteAtomsPerQuoteLot),
  );
});

test("refuses an account that is not a market", () => {
  const bytes = account();
  bytes[0] = 0;
  assert.throws(() => decodeMarket(bytes), MarketDecodeError);
});

test("refuses a version it does not understand", () => {
  // The layout is cast, not parsed. Decoding a future version would read whatever fields
  // moved as though they had not.
  const bytes = account();
  bytes[8] = 2;
  assert.throws(() => decodeMarket(bytes), /market version 2/);
});

test("refuses an account too short to hold its own parameters", () => {
  assert.throws(() => decodeMarket(account().subarray(0, DECODED_LENGTH - 1)), /at least/);
});

test("refuses a lot configuration no market could have been created with", () => {
  // A Pod cast bypasses every constructor, so a zeroed field reaches the decoder intact
  // and divides by zero in whatever prices something next.
  const bytes = account();
  const view = new DataView(bytes.buffer, bytes.byteOffset);
  view.setBigUint64(224, 0n, true);
  assert.throws(() => decodeMarket(bytes), lots.LotConfigError);
});

test("prices and sizes render in units a person reads", () => {
  const { lotConfig } = decodeMarket(account());
  // 9-decimal base, 6-decimal quote — the SOL/USDC shape the CLI creates.
  assert.equal(lots.formatPrice(lotConfig, 150_000n, 6), "150");
  assert.equal(lots.formatPrice(lotConfig, 150_250n, 6), "150.25");
  assert.equal(lots.formatSize(lotConfig, 1_000n, 9), "1");
  assert.equal(lots.formatSize(lotConfig, 1n, 9), "0.001");
});

test("a fill's quote value is exact", () => {
  const { lotConfig } = decodeMarket(account());
  // One base lot at one tick is a whole number of quote lots, by the invariant the
  // configuration is checked against. 40 lots at 150,000 ticks is exactly that, 40 times.
  const perLotPerTick = lots.quoteLotsPerBaseLotPerTick(lotConfig);
  assert.equal(perLotPerTick, 1n);
  assert.equal(lots.quoteLotsFor(lotConfig, 150_000n, 40n), 150_000n * 40n);
});

test("the affordable size rounds down, never up", () => {
  const { lotConfig } = decodeMarket(account());
  // A budget one quote lot short of two lots buys one. Rounding the other way would
  // submit an order the seat cannot pay for.
  assert.equal(lots.baseLotsFor(lotConfig, 100n, 199n), 1n);
  assert.equal(lots.baseLotsFor(lotConfig, 100n, 200n), 2n);
});

test("lots and atoms convert both ways", () => {
  const { lotConfig } = decodeMarket(account());
  assert.equal(lots.baseAtoms(lotConfig, 5n), 5_000_000n);
  assert.equal(lots.baseLotsFromAtoms(lotConfig, 5_000_000n), 5n);
  // Dust below one lot stays with the depositor rather than rounding into the market.
  assert.equal(lots.baseLotsFromAtoms(lotConfig, 5_999_999n), 5n);
});

test("quantities survive past the range a JavaScript number covers", () => {
  // The reason every quantity here is a bigint. A price is a u64, and above 2^53 a
  // double cannot hold consecutive integers: these two distinct prices collapse onto the
  // same number, so a decoder using `number` would report one order's price for another.
  assert.equal(Number(2n ** 53n), Number(2n ** 53n + 1n));

  // bigint keeps them apart, and keeps the arithmetic on them exact.
  const { lotConfig } = decodeMarket(account());
  const big = 2n ** 53n + 1n;
  assert.notEqual(big, 2n ** 53n);
  assert.equal(lots.quoteLotsFor(lotConfig, big, 1n), big);
});
