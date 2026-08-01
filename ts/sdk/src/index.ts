/**
 * TypeScript SDK for the ClobDex spot market.
 *
 * @example Building an order
 * ```ts
 * import { decodeMarket, placeOrder, postOnly, Side } from "@clobdex/sdk";
 *
 * // Every address the market uses is recorded in its own account, so reading it once is
 * // all the setup there is — no PDA derivation, no configuration file.
 * const market = decodeMarket(await fetchAccountData(marketAddress));
 * const addresses = {
 *   programAddress, market: marketAddress, tokenProgram: TOKEN_PROGRAM_ADDRESS,
 *   baseVault: market.baseVault, quoteVault: market.quoteVault, vaultSigner,
 * };
 *
 * const instruction = placeOrder(
 *   addresses,
 *   trader,
 *   postOnly(Side.Bid, 150_000n, 25n),
 * );
 * ```
 *
 * The result is plain data — a program address, an account list and a `Uint8Array` — for
 * whichever client actually sends it.
 */

export * from "./base58.ts";
export * from "./encode.ts";
export * from "./instructions.ts";
export * from "./lots.ts";
export * from "./market.ts";
export * from "./types.ts";
