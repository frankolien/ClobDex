/**
 * Everything about this deploy that is not content.
 *
 * The app lives at another origin, so every link to it is absolute and there is exactly
 * one place to change when that origin moves. `PUBLIC_` is Astro's prefix for a variable
 * that is allowed into the browser bundle; nothing here is a secret, and a marketing site
 * that needed one would be doing something wrong.
 */

/**
 * Where the trading app is deployed, or `null` if this build was not told.
 *
 * Deliberately not defaulted. The first version of this file guessed a plausible domain,
 * and that domain turned out to be registered by a squatter and parked for sale — so every
 * "Launch app" button on the site pointed at somebody's listing page, and the only symptom
 * was a TLS error nobody would connect to a missing environment variable.
 *
 * A build that does not know where its app lives should say so, not invent an address.
 */
export const appUrl: string | null = import.meta.env.PUBLIC_APP_URL ?? null;

export const links = {
  app: appUrl,
  github: "https://github.com/frankolien/ClobDex",
  benchmarks: "https://github.com/frankolien/ClobDex/blob/main/BENCHMARKS.md",
  sdk: "https://github.com/frankolien/ClobDex/tree/main/ts/sdk",
  indexer: "https://github.com/frankolien/ClobDex/tree/main/crates/clob-stream",
} as const;

/**
 * What is true today.
 *
 * Held here so the status band, the hero chip and the footer cannot drift apart and start
 * claiming different things on the same page. A visitor deciding whether to send money
 * somewhere is entitled to find the same answer everywhere they look.
 */
export const status = {
  cluster: "Devnet",
  audited: false,
  programAddress: "DaNh1Gk3xCLwzHhFQZTgLZuvUMS8YyfPqzs9ZgqFqhTe",
} as const;

/**
 * Where a "Launch app" control should point, and what it should say.
 *
 * When the app has no deployment, the honest control is one that goes to the source rather
 * than one that goes nowhere. A disabled button invites a click and answers nothing; a
 * button that navigates to a stranger's parking page is worse than either.
 */
export function launch(): { href: string; label: string; live: boolean } {
  return appUrl === null
    ? { href: links.github, label: "View the source", live: false }
    : { href: appUrl, label: "Launch app", live: true };
}
