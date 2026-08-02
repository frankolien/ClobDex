import { test } from "node:test";
import assert from "node:assert/strict";

import { AccountRole } from "@solana/kit";

import { cancelOrder, deposit, limit, placeOrder, postOnly, PostOnlyRejection, Side } from "@clobdex/sdk";
import type { MarketAddresses } from "@clobdex/sdk";

import { roleOf, signatureToString, toKit } from "../src/lib/tx.ts";

const addresses: MarketAddresses = {
  programAddress: "DaNh1GkExAEmfJ2TzKaSDPckq47uwYuDzL9aeGU9fiqK",
  market: "Co2FDvpv1111111111111111111111111111zh8ymY",
  baseVault: "BaseVau1t11111111111111111111111111111111111",
  quoteVault: "QuoteVau1t1111111111111111111111111111111111",
  vaultSigner: "VaultSigner111111111111111111111111111111111",
  tokenProgram: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
};

const trader = "Trader11111111111111111111111111111111111111";

test("the four signer/writable combinations map to the four kit roles", () => {
  // The one place the adaptation is not the identity function: the SDK describes an
  // account with two booleans and kit describes the same four cases with one enum.
  // Reversing this produces a transaction rejected for the wrong signers, or — worse —
  // one accepted having asked for write access it did not need.
  assert.equal(roleOf(false, false), AccountRole.READONLY);
  assert.equal(roleOf(false, true), AccountRole.WRITABLE);
  assert.equal(roleOf(true, false), AccountRole.READONLY_SIGNER);
  assert.equal(roleOf(true, true), AccountRole.WRITABLE_SIGNER);
});

test("an SDK instruction converts without losing anything", () => {
  const instruction = placeOrder(addresses, trader, postOnly(Side.Bid, 150_000n, 25n));
  const converted = toKit(instruction);

  assert.equal(converted.programAddress, instruction.programAddress);
  assert.equal(converted.accounts?.length, instruction.accounts.length);
  // The same bytes, not a copy that might have been re-encoded on the way through.
  assert.equal(converted.data, instruction.data);

  // Annotated once rather than inferred per lookup: kit's account type is generic over its
  // address, and indexing into it inside the loop makes the inference circular.
  const mapped: { address: string; role: AccountRole }[] = [...(converted.accounts ?? [])];
  for (const [index, account] of instruction.accounts.entries()) {
    assert.equal(mapped[index]?.address, account.address, `address ${index}`);
    assert.equal(mapped[index]?.role, roleOf(account.signer, account.writable), `role ${index}`);
  }
});

test("account order is preserved exactly", () => {
  // Solana identifies accounts by position, so a reordering is not a different-looking
  // transaction — it is a different transaction, against different accounts.
  const instruction = deposit(addresses, trader, "BaseAta1111111111111111111111111111111111111", "QuoteAta111111111111111111111111111111111111", 100n, 200n);
  const converted = toKit(instruction);

  assert.deepEqual(
    converted.accounts?.map((account: { address: string }) => account.address),
    instruction.accounts.map((account) => account.address),
  );
});

test("the trader signs and the market is writable", () => {
  // A spot check that the roles survive with their meaning, not just their shape: the
  // trader must sign, and the market account is what every order mutates.
  const instruction = placeOrder(
    addresses,
    trader,
    limit(Side.Ask, 151_000n, 10n),
  );
  const converted = toKit(instruction);

  const signer = converted.accounts?.find((account: { address: string }) => account.address === trader);
  const market = converted.accounts?.find((account: { address: string }) => account.address === addresses.market);

  assert.ok(signer, "the trader is among the accounts");
  assert.ok(
    signer.role === AccountRole.READONLY_SIGNER || signer.role === AccountRole.WRITABLE_SIGNER,
    "the trader signs",
  );
  assert.ok(market, "the market is among the accounts");
  assert.ok(
    market.role === AccountRole.WRITABLE || market.role === AccountRole.WRITABLE_SIGNER,
    "the market is writable",
  );
});

test("a cancel carries the identity the program takes", () => {
  // A bid's stored sequence number sits just below u64::MAX. It has to survive to the wire
  // exactly, which is why the whole app is bigint and the indexer quotes these as strings.
  const id = { priceInTicks: 98_000n, orderSequenceNumber: 18_446_744_073_709_551_610n };
  const converted = toKit(cancelOrder(addresses, trader, id));

  assert.ok(converted.data instanceof Uint8Array);
  assert.ok(converted.data.length > 0);
  assert.equal(converted.programAddress, addresses.programAddress);
});

test("a signature renders as base58", () => {
  const bytes = new Uint8Array(64).fill(7);
  const rendered = signatureToString(bytes);

  assert.equal(typeof rendered, "string");
  assert.ok(rendered.length > 0);
  // Base58 excludes the four characters that are easy to confuse by eye.
  assert.doesNotMatch(rendered, /[0OIl]/);
});

test("post-only and limit produce different bytes", () => {
  // Cheap guard against a builder that silently ignores its packet: if these matched, the
  // app would be sending crossing orders while the UI said post-only.
  const a = toKit(placeOrder(addresses, trader, postOnly(Side.Bid, 150_000n, 25n, PostOnlyRejection.Reject)));
  const b = toKit(placeOrder(addresses, trader, limit(Side.Bid, 150_000n, 25n)));

  assert.notDeepEqual(Array.from(a.data!), Array.from(b.data!));
});
