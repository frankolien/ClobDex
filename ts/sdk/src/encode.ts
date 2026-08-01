/**
 * Writing instruction data, byte for byte as the program reads it.
 *
 * The mirror of `Data` in `clob-client/src/instruction.rs`, and the reason
 * `spec/wire-vectors.json` exists: this is a second copy of a byte layout, and the only
 * thing keeping it honest is a test that holds it to the first.
 *
 * Everything is little-endian, which is the layout the program casts in place rather
 * than a serialisation choice.
 */

import { decodeAddress } from "./base58.ts";
import type { OrderId, OrderPacket } from "./types.ts";
import { Side } from "./types.ts";

/** Instruction discriminants, in the order the program's `Reader` parses them. */
export const Discriminant = {
  InitializeMarket: 0,
  ClaimSeat: 1,
  Deposit: 2,
  Withdraw: 3,
  PlaceOrder: 4,
  CancelOrder: 5,
  ReduceOrder: 6,
  CancelAllOrders: 7,
  CollectFees: 8,
  LogEvent: 9,
  Swap: 10,
  EvictSeat: 11,
  BatchUpdate: 12,
} as const;
export type Discriminant = (typeof Discriminant)[keyof typeof Discriminant];

/** How an order packet's kind is tagged on the wire. */
const PACKET_KIND = { limit: 0, postOnly: 1, immediateOrCancel: 2 } as const;

/** A growable little-endian byte writer. */
export class Writer {
  #bytes: number[] = [];

  constructor(discriminant?: Discriminant) {
    if (discriminant !== undefined) this.#bytes.push(discriminant);
  }

  u8(value: number): this {
    this.#bytes.push(value & 0xff);
    return this;
  }

  u32(value: number): this {
    for (let i = 0; i < 4; i++) this.#bytes.push((value >>> (8 * i)) & 0xff);
    return this;
  }

  u64(value: bigint): this {
    if (value < 0n || value > 0xffff_ffff_ffff_ffffn) {
      throw new RangeError(`${value} does not fit in a u64`);
    }
    for (let i = 0n; i < 8n; i++) this.#bytes.push(Number((value >> (8n * i)) & 0xffn));
    return this;
  }

  bytes(values: Uint8Array): this {
    for (const byte of values) this.#bytes.push(byte);
    return this;
  }

  address(value: string): this {
    return this.bytes(decodeAddress(value));
  }

  side(value: Side): this {
    return this.u8(value);
  }

  /** An order ID: its price, then its already-encoded sequence number. */
  orderId(id: OrderId): this {
    return this.u64(id.priceInTicks).u64(id.orderSequenceNumber);
  }

  /**
   * An order packet.
   *
   * Fixed-width up to a kind-specific tail, so an unpriced market order is a priced
   * packet with the price flag cleared — which is what keeps it distinguishable from a
   * limit order at price zero rather than collapsing them onto one encoding.
   */
  packet(packet: OrderPacket): this {
    const hasPrice = packet.kind === "immediateOrCancel" ? packet.priceInTicks !== null : true;
    const price =
      packet.kind === "immediateOrCancel" ? (packet.priceInTicks ?? 0n) : packet.priceInTicks;

    this.u8(PACKET_KIND[packet.kind])
      .side(packet.side)
      .u8(hasPrice ? 1 : 0)
      .u64(price)
      .u64(packet.baseLots);

    switch (packet.kind) {
      case "limit":
        return this.u8(packet.selfTradeBehavior).u32(packet.matchLimit);
      case "postOnly":
        return this.u8(packet.rejection);
      case "immediateOrCancel":
        return this.u64(packet.minBaseLotsToFill)
          .u8(packet.selfTradeBehavior)
          .u32(packet.matchLimit);
    }
  }

  finish(): Uint8Array {
    return new Uint8Array(this.#bytes);
  }
}
