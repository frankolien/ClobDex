/**
 * The shapes this SDK speaks in, and why they are its own.
 *
 * There is no dependency on a Solana client library. An instruction is three things — a
 * program, a list of accounts, and some bytes — and every JavaScript library on the
 * chain agrees about that while disagreeing about everything else. The Solana JS
 * ecosystem has changed its address type twice in recent memory; a builder that returns
 * plain data survives that, and one that returns a library's class does not.
 *
 * So an address is a base58 string, a quantity is a `bigint`, and adapting to whichever
 * client you use is a `map` at the edge.
 *
 * Quantities are `bigint` rather than `number` throughout, and not defensively: a price
 * in ticks is a `u64` on chain, and JavaScript numbers lose integer precision above
 * 2^53. A size that rounds is a size that does not match what was signed.
 */

/** A base58-encoded 32-byte address. */
export type Address = string;

/** One account an instruction reads or writes. */
export interface AccountMeta {
  readonly address: Address;
  /** Whether this account must sign the transaction. */
  readonly signer: boolean;
  /** Whether the instruction may modify it. */
  readonly writable: boolean;
}

/**
 * An instruction, ready to be wrapped by whatever client sends it.
 *
 * Field names match `@solana/kit`'s so the adaptation is usually the identity function.
 */
export interface Instruction {
  readonly programAddress: Address;
  readonly accounts: readonly AccountMeta[];
  readonly data: Uint8Array;
}

/** Which side of the book an order is on. */
export enum Side {
  Bid = 0,
  Ask = 1,
}

/** What to do when an order would match the trader's own resting liquidity. */
export enum SelfTradeBehavior {
  /** Shrink both sides by the overlap without transferring anything. No fee. */
  DecrementTake = 0,
  /** Cancel the resting order and keep matching at undiminished size. */
  CancelProvide = 1,
  /** Reject the whole order. */
  Abort = 2,
}

/** What to do when a post-only order would cross. */
export enum PostOnlyRejection {
  /** Refuse the order. */
  Reject = 0,
  /** Reprice to the best non-crossing tick and rest there. */
  Slide = 1,
}

/** Which capacities a market account was created at. */
export enum SizeClass {
  Small = 0,
  Medium = 1,
  Large = 2,
}

/**
 * The identity of a resting order.
 *
 * `orderSequenceNumber` is the *stored* value: the market's counter for asks, and its
 * bitwise complement for bids. That inversion is what makes one ascending sort produce
 * price-time priority on both sides, so the number read off the book is the number to
 * send back — deriving it from a side and a count would get bids wrong.
 */
export interface OrderId {
  readonly priceInTicks: bigint;
  readonly orderSequenceNumber: bigint;
}

/** An order as submitted. */
export type OrderPacket =
  | {
      readonly kind: "limit";
      readonly side: Side;
      readonly priceInTicks: bigint;
      readonly baseLots: bigint;
      readonly selfTradeBehavior: SelfTradeBehavior;
      /** Price levels the match may walk. Bounds the compute a fill can cost. */
      readonly matchLimit: number;
    }
  | {
      readonly kind: "postOnly";
      readonly side: Side;
      readonly priceInTicks: bigint;
      readonly baseLots: bigint;
      readonly rejection: PostOnlyRejection;
    }
  | {
      readonly kind: "immediateOrCancel";
      readonly side: Side;
      /**
       * Absent for an unpriced sweep. The encoding carries a flag for this rather than a
       * sentinel, so a market order and a limit order at price zero stay distinguishable.
       */
      readonly priceInTicks: bigint | null;
      readonly baseLots: bigint;
      readonly minBaseLotsToFill: bigint;
      readonly selfTradeBehavior: SelfTradeBehavior;
      readonly matchLimit: number;
    };

/**
 * Everything needed to address a market.
 *
 * Given rather than derived. Every one of these is either chosen by the caller or
 * recorded in the market account itself, and deriving the one that is not —
 * `vaultSigner` — needs an ed25519 on-curve check. Shipping that arithmetic to compute
 * an address the market already knows about would be a lot of code for nothing.
 */
export interface MarketAddresses {
  readonly programAddress: Address;
  readonly market: Address;
  readonly baseVault: Address;
  readonly quoteVault: Address;
  readonly vaultSigner: Address;
  readonly tokenProgram: Address;
}

/** The canonical SPL Token program. */
export const TOKEN_PROGRAM_ADDRESS: Address = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/**
 * Whether an order should emit an event receipt.
 *
 * Emission costs about 1,500 compute units and two extra accounts, so it is a choice. A
 * maker refreshing quotes already knows what it submitted; a taker or an aggregator
 * wants the receipt.
 */
export interface Receipt {
  readonly logAuthority: Address;
  readonly logAuthorityBump: number;
}
