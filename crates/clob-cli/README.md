# clob-cli

Devnet operations for a ClobDex market: create one, trade on it, read it back.

```
clob create-market
clob new-trader alice
clob fund
clob order ask 151000 600
clob --trader alice swap bid 152000 500 --receipt
clob show
```

## Why this exists

The indexer derives a trade tape from book snapshots. Nothing it produces can be
believed until it has run against a real market producing real trades, and there was no
way to make one. This is that.

Every instruction is built through `clob-client` rather than assembled here, so using
this tool also exercises the SDK — a builder that writes bytes the program rejects fails
on devnet, not in production.

## One wallet is not enough

A market with a single participant cannot produce a trade. The taker owns every resting
order it crosses, so the program removes the liquidity under its self-trade policy, no
value changes hands, and no fee is charged. `show` reports `earned 0 quote lots` and the
book still moves — which looks exactly like trading, and is not.

That is what `new-trader` is for, and why `show` prints the fee counter: it is the
cheapest way to tell a real fill from a self-trade at a glance.

## A separate workspace

`clob-cli` is excluded from the root workspace and carries its own lockfile. Cargo
resolves one dependency graph per workspace, including every member's dev-dependencies,
so litesvm — pinned, and needing wincode 0.5 — would have to agree with solana-client,
which needs wincode 0.6. They cannot. An on-chain program and an RPC client tool have no
reason to share a lockfile.

## Configuration

`RPC_URL` and `CLOB_PROGRAM_ID` come from `.env`, which is gitignored; see
`.env.example`. The signer defaults to the Solana CLI's keypair, `--keypair` overrides
it, and `--trader <name>` acts as a wallet created by `new-trader`.

Created markets are recorded in `.clob/<cluster>.json` so later commands do not need six
pubkeys pasted in. Keypairs are written beside it, never inside it.

## License

MIT
