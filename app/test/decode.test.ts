import { test } from "node:test";
import assert from "node:assert/strict";

import { DecodeError, marketSummary, openOrder, position, quantity, window } from "../src/lib/decode.ts";
import type { WireMarketSummary, WireOpenOrder, WireWindow } from "../src/lib/wire.ts";

test("a quoted u64 survives past the range a JavaScript number covers", () => {
  // The reason the indexer quotes these at all. A bid's stored sequence number is the
  // complement of the arrival counter, so it sits just below 2^64.
  const text = "18446744073709551610";
  assert.equal(quantity(text, "id"), 18446744073709551610n);

  // What it would have been as a JSON number. Compared in bigint via the string, because
  // the interesting part is that the digits differ.
  assert.notEqual(BigInt(Math.round(Number(text))).toString(), text);
});

test("a field that is not a number is an error, not a zero", () => {
  // BigInt("") is 0n and BigInt(" 12 ") is 12n. A zero here is a real price and a real
  // size, and it would be rendered as one.
  for (const bad of ["", " ", "12 ", "1.5", "0x10", "abc", "1e3"]) {
    assert.throws(() => quantity(bad, "price"), DecodeError, `for ${JSON.stringify(bad)}`);
  }
  assert.equal(quantity("-40", "change"), -40n, "a signed change is still a number");
});

function summary(overrides: Partial<WireMarketSummary> = {}): WireMarketSummary {
  return {
    market: "MKT",
    slot: 500,
    finalized_through: 498,
    base_mint: "BASE",
    quote_mint: "QUOTE",
    base_vault: "BASEVAULT",
    quote_vault: "QUOTEVAULT",
    vault_signer: "VAULTSIGNER",
    taker_fee_bps: 7,
    lots: {
      base_lots_per_base_unit: "1000",
      tick_size_in_quote_lots_per_base_unit: "1000",
      base_atoms_per_base_lot: "1000000",
      quote_atoms_per_quote_lot: "1",
    },
    best_bid_in_ticks: "98",
    best_ask_in_ticks: "102",
    spread_in_ticks: "4",
    mid_price_in_ticks: "100",
    last_price_in_ticks: "99",
    bid_orders: 2,
    ask_orders: 1,
    base_lots_deposited: "8000",
    quote_lots_deposited: "10000000",
    seats: 2,
    trades_seen: 41,
    ...overrides,
  };
}

test("a market summary decodes every field the table renders", () => {
  const decoded = marketSummary(summary());

  assert.equal(decoded.bestBidInTicks, 98n);
  assert.equal(decoded.spreadInTicks, 4n);
  assert.equal(decoded.baseLotsDeposited, 8_000n);
  assert.equal(decoded.takerFeeBps, 7, "bps stays a number");
  assert.equal(decoded.slot, 500, "a slot stays a number");
  assert.equal(decoded.lots.baseAtomsPerBaseLot, 1_000_000n);

  // The addresses a client cannot derive for itself, carried so it never has to.
  assert.equal(decoded.vaultSigner, "VAULTSIGNER");
  assert.equal(decoded.baseVault, "BASEVAULT");
});

test("an absent price decodes to null, never to zero", () => {
  // A new market with no liquidity is an ordinary state. A zero here sorts it to the top
  // or the bottom of a table and draws a chart at a price nobody quoted.
  const decoded = marketSummary(
    summary({
      best_bid_in_ticks: null,
      best_ask_in_ticks: null,
      spread_in_ticks: null,
      mid_price_in_ticks: null,
      last_price_in_ticks: null,
    }),
  );

  assert.equal(decoded.bestBidInTicks, null);
  assert.equal(decoded.midPriceInTicks, null);
  assert.equal(decoded.lastPriceInTicks, null);
});

test("an order carries both identities, and they differ on a bid", () => {
  // order_sequence_number is what CancelOrder takes; sequence_number is what a fill
  // records. A client that conflated them would cancel nothing, on bids only.
  const bid: WireOpenOrder = {
    side: "bid",
    price_in_ticks: "98",
    order_sequence_number: "18446744073709551610",
    sequence_number: "5",
    base_lots: "10",
  };
  const decoded = openOrder(bid);

  assert.equal(decoded.orderSequenceNumber, 18446744073709551610n);
  assert.equal(decoded.sequenceNumber, 5n);
  assert.notEqual(decoded.orderSequenceNumber, decoded.sequenceNumber);
});

test("a position keeps free and locked apart", () => {
  const decoded = position({
    market: "MKT",
    trader: "WALLET",
    seat: 3,
    slot: 7,
    finalized_through: 6,
    base_lots_free: "4300",
    base_lots_locked: "700",
    quote_lots_free: "8999020",
    quote_lots_locked: "980",
    orders: [],
  });

  assert.equal(decoded.baseLotsFree, 4_300n);
  assert.equal(decoded.baseLotsLocked, 700n);
  assert.equal(decoded.baseLotsFree + decoded.baseLotsLocked, 5_000n, "the deposit, intact");
  assert.equal(decoded.seat, 3, "a seat index stays a number");
});

function emptyWindow(): WireWindow {
  return {
    market: "MKT",
    from_slot: 1,
    to_slot: 216_000,
    slots: 216_000,
    open_in_ticks: null,
    high_in_ticks: null,
    low_in_ticks: null,
    close_in_ticks: null,
    change_in_ticks: null,
    vwap_in_ticks: null,
    base_lots: "0",
    quote_lots: "0",
    trades: 0,
    truncated: false,
  };
}

test("a window with no trades has no price but still has totals", () => {
  const decoded = window(emptyWindow());

  assert.equal(decoded.openInTicks, null);
  assert.equal(decoded.changeInTicks, null);
  assert.equal(decoded.baseLots, 0n);
  assert.equal(decoded.trades, 0);
});

test("a falling market decodes to a negative change", () => {
  const decoded = window({ ...emptyWindow(), change_in_ticks: "-40", open_in_ticks: "120", close_in_ticks: "80" });

  assert.equal(decoded.changeInTicks, -40n);
  assert.ok(decoded.changeInTicks < 0n);
});

test("truncation is carried through rather than dropped", () => {
  // The one field that says the totals are a floor. Losing it here would turn an
  // under-reported volume into an authoritative one.
  assert.equal(window({ ...emptyWindow(), truncated: true }).truncated, true);
});
