import { config } from "../lib/config.ts";
import { go, tradeHref, useRoute } from "../lib/router.ts";
import { shortAddress } from "../lib/format.ts";
import { useApp } from "../lib/store.ts";
import { ConnectButton } from "./Wallet.tsx";

/**
 * The one persistent chrome.
 *
 * Carries the cluster badge, which is not decoration: this app points at whatever
 * `VITE_CLUSTER` says, and someone with mainnet and devnet tabs open needs to know which
 * one they are about to sign in without reading the URL.
 */
export function Header() {
  const route = useRoute();
  const market = useApp((state) => state.market);
  const trader = useApp((state) => state.trader);
  const setTrader = useApp((state) => state.setTrader);

  const tabs = [
    { name: "Markets", href: "/", active: route.name === "markets" },
    {
      name: "Trade",
      href: market ? tradeHref(market) : "/trade",
      active: route.name === "trade",
    },
    { name: "Portfolio", href: "/portfolio", active: route.name === "portfolio" },
  ];

  /**
   * Watching an address without connecting one.
   *
   * Kept alongside the wallet button because they answer different needs: connecting is
   * for trading, and this is for looking at somebody else's position — which is how you
   * check a market maker is quoting what you think it is.
   */
  const watchAddress = () => {
    const entered = window.prompt("Wallet address to watch", trader ?? "");
    if (entered === null) return;
    setTrader(entered.trim() || null);
  };

  return (
    <header className="app-header">
      <a className="brand" href="/" onClick={navigate("/")}>
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
          <rect width="24" height="24" rx="6" fill="var(--green)" />
          <path d="M6 15h3v3H6zM6 10h3v3H6zM10.5 6h3v3h-3zM15 10h3v3h-3z" fill="#050607" />
        </svg>
        <span>ClobDex</span>
      </a>

      <nav>
        {tabs.map((tab) => (
          <a
            key={tab.name}
            href={tab.href}
            className={tab.active ? "tab on" : "tab"}
            onClick={navigate(tab.href)}
          >
            {tab.name}
          </a>
        ))}
      </nav>

      <div className="right">
        <span className={`cluster ${config.cluster}`}>{config.cluster}</span>
        <button type="button" className="btn" onClick={watchAddress} title="Look at any wallet's position without connecting">
          {trader ? shortAddress(trader) : "Watch"}
        </button>
        <ConnectButton />
        {/* Only when this build was told where the site is. An anchor with no href is a
            button that looks clickable and does nothing, which is worse than no button. */}
        {config.siteUrl && (
          <a className="btn" href={config.siteUrl}>
            About
          </a>
        )}
      </div>
    </header>
  );
}

/** Intercepts a left click so navigation stays in-page, and leaves every other click alone. */
function navigate(href: string) {
  return (event: React.MouseEvent) => {
    // Modifier clicks open tabs and windows. Swallowing those would break the one
    // interaction a trader uses most: opening two markets side by side.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    go(href);
  };
}
