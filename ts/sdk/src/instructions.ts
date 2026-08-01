/**
 * Typed instruction builders.
 *
 * Every instruction the program accepts, built from arguments rather than assembled by
 * hand. The mirror of `clob-client/src/instruction.rs`, and checked against it by
 * `test/vectors.test.ts` — a builder that writes bytes the program cannot read is a
 * failing test here rather than a reverted transaction on chain.
 *
 * Account order is part of the interface: the program indexes into the list rather than
 * searching it, so a reordered list is a different instruction.
 */

import { Discriminant, Writer } from "./encode.ts";
import type {
  AccountMeta,
  Address,
  Instruction,
  MarketAddresses,
  OrderId,
  OrderPacket,
  Receipt,
} from "./types.ts";
import { PostOnlyRejection, SelfTradeBehavior, Side, SizeClass } from "./types.ts";

const writable = (address: Address, signer = false): AccountMeta => ({
  address,
  signer,
  writable: true,
});
const readonly = (address: Address, signer = false): AccountMeta => ({
  address,
  signer,
  writable: false,
});

/** Appends the two accounts a receipt needs. */
function receiptAccounts(
  accounts: AccountMeta[],
  market: MarketAddresses,
  receipt: Receipt | undefined,
): void {
  if (receipt) {
    accounts.push(readonly(receipt.logAuthority), readonly(market.programAddress));
  }
}

/** Appends the log-authority bump when a receipt was asked for. */
function receiptData(writer: Writer, receipt: Receipt | undefined): Writer {
  return receipt ? writer.u8(receipt.logAuthorityBump) : writer;
}

/**
 * Claims a seat, or returns the existing one.
 *
 * Idempotent, so it is safe to send before a first order without reading the market to
 * find out whether a seat already exists.
 */
export function claimSeat(market: MarketAddresses, trader: Address): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [writable(market.market), readonly(trader, true)],
    data: new Writer(Discriminant.ClaimSeat).finish(),
  };
}

/**
 * Returns an empty seat to the market, making room for a new trader.
 *
 * Permissionless, and correctly so: it only succeeds on a seat holding nothing at all,
 * and losing an empty seat costs its owner nothing because claiming is free. Requiring a
 * signature would put the market's liveness in the hands of people who have stopped
 * participating.
 */
export function evictSeat(market: MarketAddresses, trader: Address): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [writable(market.market), readonly(trader)],
    data: new Writer(Discriminant.EvictSeat).finish(),
  };
}

/**
 * Moves tokens in and credits a seat.
 *
 * Amounts are in lots, which convert to atoms exactly — atoms would round down and
 * strand the remainder in the vault.
 */
export function deposit(
  market: MarketAddresses,
  trader: Address,
  traderBase: Address,
  traderQuote: Address,
  baseLots: bigint,
  quoteLots: bigint,
): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [
      writable(market.market),
      readonly(trader, true),
      writable(traderBase),
      writable(traderQuote),
      writable(market.baseVault),
      writable(market.quoteVault),
      readonly(market.tokenProgram),
    ],
    data: new Writer(Discriminant.Deposit).u64(baseLots).u64(quoteLots).finish(),
  };
}

/**
 * Debits a seat and moves tokens out.
 *
 * Only free balances can leave; funds locked behind resting orders have to be cancelled
 * first.
 */
export function withdraw(
  market: MarketAddresses,
  trader: Address,
  traderBase: Address,
  traderQuote: Address,
  baseLots: bigint,
  quoteLots: bigint,
): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [
      writable(market.market),
      readonly(trader, true),
      writable(traderBase),
      writable(traderQuote),
      writable(market.baseVault),
      writable(market.quoteVault),
      readonly(market.vaultSigner),
      readonly(market.tokenProgram),
    ],
    data: new Writer(Discriminant.Withdraw).u64(baseLots).u64(quoteLots).finish(),
  };
}

/** Submits an order against funds already on the market. */
export function placeOrder(
  market: MarketAddresses,
  trader: Address,
  packet: OrderPacket,
  receipt?: Receipt,
): Instruction {
  const accounts = [writable(market.market), readonly(trader, true)];
  receiptAccounts(accounts, market, receipt);

  return {
    programAddress: market.programAddress,
    accounts,
    data: receiptData(new Writer(Discriminant.PlaceOrder).packet(packet), receipt).finish(),
  };
}

/** Cancels a resting order and releases its backing funds. */
export function cancelOrder(
  market: MarketAddresses,
  trader: Address,
  orderId: OrderId,
): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [writable(market.market), readonly(trader, true)],
    data: new Writer(Discriminant.CancelOrder).orderId(orderId).finish(),
  };
}

/** Shrinks a resting order without giving up its queue position. */
export function reduceOrder(
  market: MarketAddresses,
  trader: Address,
  orderId: OrderId,
  baseLots: bigint,
): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [writable(market.market), readonly(trader, true)],
    data: new Writer(Discriminant.ReduceOrder).orderId(orderId).u64(baseLots).finish(),
  };
}

/**
 * Cancels up to `limit` of the caller's orders on one side.
 *
 * The bound is the caller's to set: an unbounded cancel-all on a deep book can exceed the
 * compute budget and revert, which is the worst possible moment to fail.
 */
export function cancelAllOrders(
  market: MarketAddresses,
  trader: Address,
  side: Side,
  limit: number,
): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [writable(market.market), readonly(trader, true)],
    data: new Writer(Discriminant.CancelAllOrders).side(side).u32(limit).finish(),
  };
}

/**
 * Cancels a set of orders and places a set of orders, in one instruction.
 *
 * The market-maker cycle. Cancels run first, because releasing the capital behind stale
 * quotes is what funds the new ones. A cancel that finds nothing is not an error — the
 * order filled, which is the ordinary case rather than a failure. Placement is
 * all-or-nothing.
 *
 * There is no receipt form: the receipt exists for takers and aggregators, and a maker
 * already knows what it submitted.
 *
 * @throws if either list is longer than 255, which is all the length prefix can express.
 */
export function batchUpdate(
  market: MarketAddresses,
  trader: Address,
  cancels: readonly OrderId[],
  orders: readonly OrderPacket[],
): Instruction {
  if (cancels.length > 255 || orders.length > 255) {
    throw new RangeError(
      `a batch carries at most 255 of each, got ${cancels.length} cancels and ${orders.length} orders`,
    );
  }

  const writer = new Writer(Discriminant.BatchUpdate).u8(cancels.length);
  for (const id of cancels) writer.orderId(id);
  writer.u8(orders.length);
  for (const packet of orders) writer.packet(packet);

  return {
    programAddress: market.programAddress,
    accounts: [writable(market.market), readonly(trader, true)],
    data: writer.finish(),
  };
}

/**
 * Sweeps accrued fees to the market's fee recipient.
 *
 * Permissionless — the recipient is fixed in the market header, so there is nothing to
 * gain by calling it.
 */
export function collectFees(market: MarketAddresses, feeRecipient: Address): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [
      writable(market.market),
      writable(market.quoteVault),
      writable(feeRecipient),
      readonly(market.vaultSigner),
      readonly(market.tokenProgram),
    ],
    data: new Writer(Discriminant.CollectFees).finish(),
  };
}

/**
 * Deposits, matches and withdraws in one instruction, for a caller holding no balance on
 * the market.
 *
 * A swap must be priced: the program moves in the most the order could cost, and an
 * unpriced market buy has no bounded cost.
 */
export function swap(
  market: MarketAddresses,
  trader: Address,
  traderBase: Address,
  traderQuote: Address,
  args: {
    side: Side;
    priceInTicks: bigint;
    baseLots: bigint;
    minBaseLotsToFill: bigint;
    matchLimit: number;
  },
  receipt?: Receipt,
): Instruction {
  const accounts = [
    writable(market.market),
    readonly(trader, true),
    writable(traderBase),
    writable(traderQuote),
    writable(market.baseVault),
    writable(market.quoteVault),
    readonly(market.vaultSigner),
    readonly(market.tokenProgram),
  ];
  receiptAccounts(accounts, market, receipt);

  const writer = new Writer(Discriminant.Swap)
    .side(args.side)
    .u64(args.priceInTicks)
    .u64(args.baseLots)
    .u64(args.minBaseLotsToFill)
    .u32(args.matchLimit);

  return {
    programAddress: market.programAddress,
    accounts,
    data: receiptData(writer, receipt).finish(),
  };
}

/**
 * Creates a market in an account the caller has already allocated.
 *
 * Allocation stays with the caller because it needs the payer's signature on a
 * `CreateAccount` from the system program; this instruction validates the size it was
 * given rather than choosing it.
 */
export function initializeMarket(
  market: MarketAddresses,
  args: {
    baseMint: Address;
    quoteMint: Address;
    authority: Address;
    feeRecipient: Address;
    sizeClass: SizeClass;
    baseLotsPerBaseUnit: bigint;
    tickSizeInQuoteLotsPerBaseUnit: bigint;
    baseAtomsPerBaseLot: bigint;
    quoteAtomsPerQuoteLot: bigint;
    takerFeeBps: bigint;
    vaultSignerBump: number;
  },
): Instruction {
  return {
    programAddress: market.programAddress,
    accounts: [
      writable(market.market),
      readonly(args.baseMint),
      readonly(args.quoteMint),
      readonly(market.baseVault),
      readonly(market.quoteVault),
      readonly(market.vaultSigner),
      readonly(args.authority, true),
      readonly(args.feeRecipient),
    ],
    data: new Writer(Discriminant.InitializeMarket)
      .u64(BigInt(args.sizeClass))
      .u64(args.baseLotsPerBaseUnit)
      .u64(args.tickSizeInQuoteLotsPerBaseUnit)
      .u64(args.baseAtomsPerBaseLot)
      .u64(args.quoteAtomsPerQuoteLot)
      .u64(args.takerFeeBps)
      .u8(args.vaultSignerBump)
      .finish(),
  };
}

// ---------------------------------------------------------------------------------
// Convenience constructors for the common order shapes
// ---------------------------------------------------------------------------------

/** A good-till-cancelled limit order that crosses first and rests the remainder. */
export function limit(side: Side, priceInTicks: bigint, baseLots: bigint): OrderPacket {
  return {
    kind: "limit",
    side,
    priceInTicks,
    baseLots,
    selfTradeBehavior: SelfTradeBehavior.DecrementTake,
    matchLimit: 0xffff_ffff,
  };
}

/** A quote that rests or is refused, never taking liquidity. */
export function postOnly(
  side: Side,
  priceInTicks: bigint,
  baseLots: bigint,
  rejection: PostOnlyRejection = PostOnlyRejection.Reject,
): OrderPacket {
  return { kind: "postOnly", side, priceInTicks, baseLots, rejection };
}

/** An unpriced sweep that keeps whatever it fills. */
export function marketOrder(side: Side, baseLots: bigint, matchLimit: number): OrderPacket {
  return {
    kind: "immediateOrCancel",
    side,
    priceInTicks: null,
    baseLots,
    minBaseLotsToFill: 0n,
    selfTradeBehavior: SelfTradeBehavior.DecrementTake,
    matchLimit,
  };
}

/** An all-or-nothing immediate order. */
export function fillOrKill(side: Side, priceInTicks: bigint, baseLots: bigint): OrderPacket {
  return {
    kind: "immediateOrCancel",
    side,
    priceInTicks,
    baseLots,
    minBaseLotsToFill: baseLots,
    selfTradeBehavior: SelfTradeBehavior.DecrementTake,
    matchLimit: 0xffff_ffff,
  };
}
