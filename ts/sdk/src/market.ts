/**
 * Reading a market account.
 *
 * # What this decodes, and what it does not
 *
 * The first 296 bytes: the account preamble, then the engine's own header. That is every
 * address the market uses, its tick and lot geometry, its fee rate, and its running
 * totals — flat fields at fixed offsets, no traversal.
 *
 * The book and the seat table are not decoded here. They are red-black trees in a Pod
 * arena, and walking one in TypeScript would be a second implementation of a data
 * structure whose correctness the Rust side proves with property tests. The indexer
 * already decodes it and serves aggregated levels as JSON, which is what a UI wants
 * anyway — a price and a size, not an arena of nodes.
 *
 * So: use this for the market's parameters, which are static and worth verifying against
 * the chain yourself. Use the indexer for the book, which changes every slot.
 *
 * # Why the addresses come from here
 *
 * Recording the vaults in the account is a compute optimisation on chain — verifying a
 * vault becomes a 32-byte comparison instead of a PDA search. Off chain it has a second
 * effect: a client that has read the account needs to derive nothing, which is why this
 * SDK ships no elliptic-curve arithmetic.
 */

import { encodeBase58 } from "./base58.ts";
import type { LotConfig } from "./lots.ts";
import { validate } from "./lots.ts";
import type { Address } from "./types.ts";
import { SizeClass } from "./types.ts";

/** Marks an account as a ClobDex market: the first eight bytes. */
export const MARKET_DISCRIMINATOR = "434c4f424d4b5431";

/** The account format this SDK understands. */
export const MARKET_VERSION = 1n;

/** Bytes of preamble before the engine's market begins. */
export const ACCOUNT_HEADER_LENGTH = 224;

/** Bytes this decoder needs: the preamble plus the engine's own header. */
export const DECODED_LENGTH = ACCOUNT_HEADER_LENGTH + 72;

/** Why a market account could not be decoded. */
export class MarketDecodeError extends Error {}

/** A market's parameters and running totals, as recorded on chain. */
export interface Market {
  /** Which capacities this account was created at. */
  readonly sizeClass: SizeClass;
  /** Bump for the vault signer, recorded so it never has to be searched for. */
  readonly vaultSignerBump: number;
  readonly baseMint: Address;
  readonly quoteMint: Address;
  readonly baseVault: Address;
  readonly quoteVault: Address;
  /** May change the fee recipient. Cannot touch trader funds. */
  readonly authority: Address;
  /** Receives swept fees. */
  readonly feeRecipient: Address;
  /** Tick and lot geometry. Immutable after creation. */
  readonly lotConfig: LotConfig;
  /** Taker fee, in basis points. Makers pay nothing. */
  readonly takerFeeBps: bigint;
  /** Base lots the market holds on behalf of all seats. */
  readonly baseLotsDeposited: bigint;
  /** Quote lots the market holds, including unclaimed fees. */
  readonly quoteLotsDeposited: bigint;
  /** Lifetime fees earned. Never decreases, so it survives a sweep. */
  readonly collectedQuoteLotFees: bigint;
  /** Fees earned but not yet swept. */
  readonly unclaimedQuoteLotFees: bigint;
}

/**
 * Decodes a market account's parameters.
 *
 * @throws {MarketDecodeError} if the buffer is too short, is not a market, or was
 * written by an incompatible version.
 */
export function decodeMarket(data: Uint8Array): Market {
  if (data.length < DECODED_LENGTH) {
    throw new MarketDecodeError(
      `account is ${data.length} bytes, need at least ${DECODED_LENGTH} to read its parameters`,
    );
  }

  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const u64 = (offset: number) => view.getBigUint64(offset, true);
  const address = (offset: number): Address => encodeBase58(data.subarray(offset, offset + 32));

  const discriminator = [...data.subarray(0, 8)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
  if (discriminator !== MARKET_DISCRIMINATOR) {
    throw new MarketDecodeError("not a market account");
  }

  const version = u64(8);
  if (version !== MARKET_VERSION) {
    throw new MarketDecodeError(`market version ${version}, this SDK understands ${MARKET_VERSION}`);
  }

  const sizeClassValue = u64(16);
  const sizeClass = ([SizeClass.Small, SizeClass.Medium, SizeClass.Large] as const)[
    Number(sizeClassValue)
  ];
  if (sizeClass === undefined) {
    throw new MarketDecodeError(`unknown size class ${sizeClassValue}`);
  }

  const body = ACCOUNT_HEADER_LENGTH;
  const lotConfig: LotConfig = {
    baseLotsPerBaseUnit: u64(body),
    tickSizeInQuoteLotsPerBaseUnit: u64(body + 8),
    baseAtomsPerBaseLot: u64(body + 16),
    quoteAtomsPerQuoteLot: u64(body + 24),
  };

  // The bytes were cast, not parsed, so nothing has checked these numbers. Refusing here
  // means anything that survives decoding is safe to price with; the alternative is a
  // division by zero in whatever reads it next.
  validate(lotConfig);

  return {
    sizeClass,
    vaultSignerBump: Number(u64(24)),
    baseMint: address(32),
    quoteMint: address(64),
    baseVault: address(96),
    quoteVault: address(128),
    authority: address(160),
    feeRecipient: address(192),
    lotConfig,
    takerFeeBps: u64(body + 32),
    baseLotsDeposited: u64(body + 40),
    quoteLotsDeposited: u64(body + 48),
    collectedQuoteLotFees: u64(body + 56),
    unclaimedQuoteLotFees: u64(body + 64),
  };
}
