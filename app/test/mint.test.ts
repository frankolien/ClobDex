import { test } from "node:test";
import assert from "node:assert/strict";

import { MINT_LENGTH, MintDecodeError, decimalsOf, fromBase64 } from "../src/lib/mint.ts";

/** An SPL mint account, laid out as the token program writes it. */
function mint({ decimals = 9, initialized = true, length = MINT_LENGTH } = {}): Uint8Array {
  const data = new Uint8Array(length);
  data[0] = 1; // mint authority present
  data[44] = decimals;
  data[45] = initialized ? 1 : 0;
  return data;
}

test("decimals come from the byte the token program writes them to", () => {
  for (const decimals of [0, 6, 8, 9, 18]) {
    assert.equal(decimalsOf(mint({ decimals })), decimals, `for ${decimals}`);
  }
});

test("a Token-2022 mint with extensions reads the same", () => {
  // Extensions are appended after the base layout, so the first 82 bytes are unchanged and
  // this decoder does not need to know which token program owns the account.
  assert.equal(decimalsOf(mint({ decimals: 6, length: 200 })), 6);
});

test("an uninitialised mint is refused rather than read as zero decimals", () => {
  // A zeroed account decodes to a perfectly plausible "0 decimals", and displaying every
  // amount a billion times too large is worse than displaying nothing at all.
  assert.throws(() => decimalsOf(mint({ initialized: false })), MintDecodeError);
});

test("an account too short to be a mint is refused", () => {
  assert.throws(() => decimalsOf(new Uint8Array(40)), MintDecodeError);
  assert.throws(() => decimalsOf(mint({ length: MINT_LENGTH - 1 })), MintDecodeError);
});

test("an implausible exponent is refused", () => {
  // No SPL mint has more than nine in practice and the field is a u8, so a large value
  // means this is not the account anyone thought it was.
  assert.throws(() => decimalsOf(mint({ decimals: 200 })), MintDecodeError);
});

test("base64 account data round-trips byte for byte", () => {
  const original = mint({ decimals: 6 });
  const encoded = btoa(String.fromCharCode(...original));
  assert.deepEqual(fromBase64(encoded), original);
  assert.equal(decimalsOf(fromBase64(encoded)), 6);
});
