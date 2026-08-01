import { test } from "node:test";
import assert from "node:assert/strict";

import { SLOTS_PER_DAY, SLOT_MS, ago, anchorAt, secondsOf, slotOf, slotsIn, timeOf } from "../src/lib/time.ts";

const NOON = Date.UTC(2026, 0, 1, 12, 0, 0);
const anchor = anchorAt(1_000_000, NOON);

test("a slot converts to a time and back", () => {
  assert.equal(timeOf(anchor, 1_000_000), NOON);
  assert.equal(timeOf(anchor, 1_000_010), NOON + 10 * SLOT_MS);
  assert.equal(timeOf(anchor, 999_990), NOON - 10 * SLOT_MS);

  for (const slot of [999_000, 1_000_000, 1_000_001, 1_002_500]) {
    assert.equal(slotOf(anchor, timeOf(anchor, slot)), slot, `for slot ${slot}`);
  }
});

test("the error is measured from the anchor, not from the epoch", () => {
  // Why conversions take an anchor at all. Pinning the newest slot seen to now makes a
  // chart's right-hand edge exact and lets the drift accumulate leftwards, where a label
  // being a few seconds out is invisible.
  assert.equal(timeOf(anchor, anchor.slot), anchor.unixMs, "no error at the anchor");

  const later = anchorAt(anchor.slot + SLOTS_PER_DAY, NOON + 86_400_000);
  assert.equal(timeOf(later, later.slot), later.unixMs);
});

test("a day of slots is the same number the indexer defaults to", () => {
  // The two have to agree: this constant labels an axis "24h" and that one selects which
  // trades the window counted. Disagreeing would put a label on the wrong data.
  assert.equal(SLOTS_PER_DAY, 216_000);
  assert.equal(slotsIn(86_400_000), SLOTS_PER_DAY);
});

test("a span is never zero slots", () => {
  // A zero-slot window is a span containing nothing, and asking for one is always a
  // mistake in the caller rather than a request to be honoured.
  assert.equal(slotsIn(0), 1);
  assert.equal(slotsIn(-5), 1);
  assert.equal(slotsIn(100), 1, "under one slot still spans one");
});

test("a chart axis takes whole seconds", () => {
  assert.equal(secondsOf(anchor, 1_000_000), Math.floor(NOON / 1000));
  assert.equal(secondsOf(anchor, 1_000_005), Math.floor((NOON + 2_000) / 1000));
});

test("elapsed time is deliberately coarse", () => {
  // Rendering "1.4s ago" from an approximation claims a precision it does not have.
  const now = (slots: number) => timeOf(anchor, anchor.slot + slots);

  assert.equal(ago(anchor, anchor.slot, now(0)), "just now");
  assert.equal(ago(anchor, anchor.slot, now(5)), "just now", "two seconds is still just now");
  assert.equal(ago(anchor, anchor.slot, now(75)), "30s ago");
  assert.equal(ago(anchor, anchor.slot, now(300)), "2m ago");
  assert.equal(ago(anchor, anchor.slot, now(9_000)), "1h ago");
  assert.equal(ago(anchor, anchor.slot, now(SLOTS_PER_DAY * 3)), "3d ago");
});

test("a slot from the future reads as now rather than as negative time", () => {
  // The indexer runs at `confirmed` and this clock is an estimate, so a fill can arrive
  // stamped very slightly ahead. "in -2s" is a bug report; "just now" is the truth.
  assert.equal(ago(anchor, anchor.slot + 50, timeOf(anchor, anchor.slot)), "just now");
});
