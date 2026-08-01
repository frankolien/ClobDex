# @clobdex/sdk

TypeScript for the ClobDex spot market: instruction builders, market decoding, and exact
lot math. No runtime dependencies.

```ts
import { decodeMarket, placeOrder, postOnly, Side, TOKEN_PROGRAM_ADDRESS } from "@clobdex/sdk";

const market = decodeMarket(accountData);

const instruction = placeOrder(
  {
    programAddress,
    market: marketAddress,
    baseVault: market.baseVault,
    quoteVault: market.quoteVault,
    vaultSigner,
    tokenProgram: TOKEN_PROGRAM_ADDRESS,
  },
  trader,
  postOnly(Side.Bid, 150_000n, 25n),
);
```

`instruction` is plain data — a program address, an account list, and a `Uint8Array` — for
whichever client sends it.

## It cannot drift from the program

The Rust SDK avoids two copies of the wire format by depending on the program crate
directly: the byte layout exists once. TypeScript cannot do that. It has to re-implement
the encoding, which is exactly the failure the Rust builders warn about in their own docs:

> Two copies of a byte layout is how a client and a program drift apart; one copy cannot.

Since the second copy is unavoidable, drift is made into a test failure. Rust writes every
instruction — arguments and exact bytes — to [`spec/wire-vectors.json`](../../spec/wire-vectors.json),
and asserts the file still describes what it produces. This package asserts the same
arguments produce the same bytes, and the same account list, in order, with every signer
and writable flag. Neither side can move without the other going red.

```
cargo test -p clob-client --test wire_vectors   # Rust still matches the file
npm test                                        # TypeScript still matches the file
```

A deliberate format change regenerates the file with `UPDATE_VECTORS=1` and shows up in the
diff as what it is: a change every implementation has to make.

## No dependencies, and what that costs

An instruction is a program, a list of accounts, and some bytes. Every JavaScript library
on this chain agrees about that and disagrees about everything else — the ecosystem has
changed its address type twice in recent memory. Returning plain data survives that; the
field names match `@solana/kit`'s, so adapting is usually the identity function.

Base58 is written out here rather than taken from a package. It is forty lines of
arithmetic, and a wallet address is the last thing that should pass through code nobody in
this repository has read.

**The cost:** no PDA derivation. Deriving the vault signer needs an ed25519 on-curve check,
and shipping that arithmetic would be a lot of code for one address — so addresses are
given, not derived. This turns out to be free in practice: the market account records its
own mints, vaults and vault-signer bump, so reading the account once supplies everything
except the vault signer itself, which `create-market` already found and recorded.

## What it decodes, and what it does not

`decodeMarket` reads the first 296 bytes: every address, the tick and lot geometry, the fee
rate, and the running totals.

It does not decode the book or the seat table. Those are red-black trees in a `Pod` arena,
and walking one here would be a second implementation of a data structure the Rust property
tests already cover. The indexer decodes it and serves aggregated price levels as JSON,
which is what a UI wants anyway — a price and a size, not an arena of nodes.

So: this package for the market's parameters, which are static and worth verifying against
the chain yourself. The indexer for the book, which changes every slot.

## Everything is `bigint`

Not defensively. A price in ticks is a `u64` on chain, and a JavaScript number cannot hold
consecutive integers above 2^53:

```ts
Number(2n ** 53n) === Number(2n ** 53n + 1n);  // true
```

Two distinct prices, one number. A size that rounds is a size that does not match what was
signed.

The one place a fraction is correct is display, where a person reads `150.25` rather than
`150250` ticks. `formatPrice` and `formatSize` return strings for exactly that, and return
strings so the value cannot be fed back into arithmetic that expects exactness.

## Developing

Node runs the TypeScript directly, so there is no build step to work against:

```
npm test        # node --test, no bundler, no transpile
npm run typecheck
npm run build   # only to emit .d.ts and .js for consumers
```

`enum` is not available: it emits runtime code, so Node cannot erase it, and
`erasableSyntaxOnly` makes the type checker say so rather than the test runner. Constants
are frozen objects with a matching type, which reads the same at the call site.
