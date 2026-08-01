import { test } from "node:test";
import assert from "node:assert/strict";

import { decodeAddress, decodeBase58, encodeBase58 } from "../src/base58.ts";
import vectors from "../../../spec/wire-vectors.json" with { type: "json" };

test("round-trips every byte value", () => {
  const bytes = new Uint8Array(256);
  for (let i = 0; i < 256; i++) bytes[i] = i;
  assert.deepEqual(decodeBase58(encodeBase58(bytes)), bytes);
});

test("agrees with Rust on the addresses in the wire vectors", () => {
  // The fixtures name their addresses as repeated bytes — the program is [9; 32], the
  // market [1; 32] — so Rust's base58 of those is an oracle this implementation has to
  // match. Anything hand-written here would be a guess being checked against itself.
  const repeated = (byte: number) => new Uint8Array(32).fill(byte);
  assert.equal(encodeBase58(repeated(9)), vectors.inputs.programAddress);
  assert.equal(encodeBase58(repeated(1)), vectors.inputs.market);
  assert.equal(encodeBase58(repeated(100)), vectors.inputs.trader);
  assert.deepEqual(decodeAddress(vectors.inputs.programAddress), repeated(9));
});

test("the all-zero address is thirty-two ones", () => {
  // Leading zeros carry no magnitude, so they only survive if counted separately. An
  // encoder that forgets turns the system program into an empty string.
  const zero = new Uint8Array(32);
  assert.equal(encodeBase58(zero), "1".repeat(32));
  assert.deepEqual(decodeBase58("1".repeat(32)), zero);
});

test("keeps leading zeros through a round trip", () => {
  const bytes = new Uint8Array([0, 0, 0, 7, 255, 1]);
  assert.deepEqual(decodeBase58(encodeBase58(bytes)), bytes);
});

test("an empty input stays empty", () => {
  assert.equal(encodeBase58(new Uint8Array(0)), "");
  assert.deepEqual(decodeBase58(""), new Uint8Array(0));
});

test("refuses characters outside the alphabet", () => {
  // 0, O, I and l are excluded precisely because they are misread. Accepting them would
  // decode a mistyped address into a real one.
  for (const bad of ["0", "O", "I", "l"]) {
    assert.throws(() => decodeBase58(`1111${bad}`), /not base58/);
  }
});

test("refuses an address that is not thirty-two bytes", () => {
  assert.throws(() => decodeAddress("11"), /decodes to .* bytes/);
  assert.throws(() => decodeAddress(encodeBase58(new Uint8Array(31).fill(3))), /expected 32/);
});
