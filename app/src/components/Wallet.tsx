import { useEffect, useMemo } from "react";
import { WalletUi, WalletUiDropdown, createWalletUiConfig, useWalletUiAccount } from "@wallet-ui/react";
import { createSolanaDevnet, createSolanaLocalnet, createSolanaMainnet } from "@wallet-ui/core";

import { config } from "../lib/config.ts";
import { useApp } from "../lib/store.ts";

/**
 * Wallet connection, on the Wallet Standard.
 *
 * The SDK returns plain data whose field names already match `@solana/kit`'s, so this path
 * is the one where adapting costs almost nothing — see `lib/tx.ts` for the "almost".
 *
 * Exactly one cluster is offered: the one this deploy is built against. A picker would let
 * someone connect to mainnet on a page whose indexer and program address point at devnet,
 * and every number on screen would then belong to a different chain than the wallet does.
 */
// All three take this deploy's own RPC. Letting the wallet default to a different endpoint
// than the app reads from produces the worst kind of confusion: an order that landed but
// cannot be seen, because the node being polled has not caught up to the one that took it.
const clusters = [
  config.cluster === "mainnet"
    ? createSolanaMainnet(config.rpcUrl)
    : config.cluster === "localnet"
      ? createSolanaLocalnet(config.rpcUrl)
      : createSolanaDevnet(config.rpcUrl),
];

export function WalletProvider({ children }: { children: React.ReactNode }) {
  const walletConfig = useMemo(() => createWalletUiConfig({ clusters }), []);
  return (
    <WalletUi config={walletConfig}>
      <TrackAccount />
      {children}
    </WalletUi>
  );
}

/**
 * Mirrors the connected account into the app store.
 *
 * The store is what the read-only views already key off, so connecting a wallet simply
 * fills in the address they were waiting for. Keeping the two in sync here rather than
 * threading the wallet through every component means nothing below has to know whether an
 * address came from a wallet or from being typed in.
 */
function TrackAccount() {
  const { account } = useWalletUiAccount();
  const trader = useApp((state) => state.trader);
  const setTrader = useApp((state) => state.setTrader);

  useEffect(() => {
    const address = account?.address ?? null;
    // Only when it actually changed: `setTrader` clears the cached position, and doing
    // that on every render would empty the panel between every poll.
    if (address !== trader) setTrader(address);
    // `trader` is deliberately absent from the deps — including it would re-run this when
    // someone types an address by hand and immediately overwrite it with the wallet's.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account?.address, setTrader]);

  return null;
}

export function ConnectButton() {
  return <WalletUiDropdown label="Connect" />;
}

/** Whether a wallet is connected, for anything that has to be disabled until one is. */
export function useWallet() {
  const { account } = useWalletUiAccount();
  return { account, connected: account !== undefined };
}
