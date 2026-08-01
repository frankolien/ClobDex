/**
 * Everything about this deploy that is not content.
 *
 * The app lives at another origin, so every link to it is absolute and there is exactly
 * one place to change when that origin moves. `PUBLIC_` is Astro's prefix for a variable
 * that is allowed into the browser bundle; nothing here is a secret, and a marketing site
 * that needed one would be doing something wrong.
 */

export const links = {
  /** The trading app. A separate deploy — see `app/`. */
  app: import.meta.env.PUBLIC_APP_URL ?? "https://app.clobdex.xyz",
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
