/**
 * Routing, in about forty lines.
 *
 * Three routes and one query parameter. A router library would bring a data-loading model,
 * nested layouts and a matcher this app has no use for, and would be several times the
 * size of everything it routed.
 *
 * The market is a query parameter rather than a path segment on purpose: markets are
 * permissionless, so there is no list of them at build time, and a static export cannot
 * prerender `/trade/[address]` for addresses that do not exist yet.
 */

import { useSyncExternalStore } from "react";

export type Route =
  | { name: "markets" }
  | { name: "trade"; market: string | null }
  | { name: "portfolio" };

const listeners = new Set<() => void>();

function announce(): void {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  // Back and forward are the browser's, and a router that ignores them turns the back
  // button into a way to leave the site.
  window.addEventListener("popstate", announce);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) window.removeEventListener("popstate", announce);
  };
}

/** The current URL, as a string, so `useSyncExternalStore` can compare it cheaply. */
function snapshot(): string {
  return window.location.pathname + window.location.search;
}

/** The route this URL means. Anything unrecognised is the markets list. */
export function parse(href: string): Route {
  const url = new URL(href, window.location.origin);
  switch (url.pathname.replace(/\/+$/, "")) {
    case "/trade":
      return { name: "trade", market: url.searchParams.get("market") };
    case "/portfolio":
      return { name: "portfolio" };
    default:
      return { name: "markets" };
  }
}

export function useRoute(): Route {
  // Server snapshot is the same function: this app never renders anywhere but a browser,
  // and returning something different would be inventing a state that cannot occur.
  return parse(useSyncExternalStore(subscribe, snapshot, snapshot));
}

/** Navigates without a reload, and tells everyone listening. */
export function go(href: string): void {
  if (href === snapshot()) return;
  window.history.pushState(null, "", href);
  announce();
}

/** The href for one market's trade view. */
export function tradeHref(market: string): string {
  return `/trade?market=${encodeURIComponent(market)}`;
}
